use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
    time::SystemTime,
};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    assets::Engine,
    build_artifact, dataset,
    doctor::{DoctorProgressEvent, capture_host_environment_system},
    expected_cache::{
        CachedExpectedResult, DatasetReceiptId, ExpectedCache, ExpectedCacheKey,
        ExpectedCacheLookup, ExpectedResult, ReferenceArtifact, ReferenceArtifactId,
        ReferenceSettingValue, ReservationOutcome,
    },
    hashing,
    model::{PinPolicy, ReferenceIdentity, RunPlan},
    progress::Reporter,
    run_bundle::{
        ConfigIdentity, ExpectedResultRecord, MeasurementRecord, RunBundle, RunRecord,
        ValidationRecord, ValidationStatus,
    },
    validation::{self, VALIDATION_PROTOCOL_VERSION},
    worker::Worker,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSummary {
    pub status: String,
    pub benchmark: String,
    pub bundle: std::path::PathBuf,
    pub measurement_count: usize,
    pub validation_status: ValidationStatus,
    pub validation_mismatches: usize,
    pub expected_cache_hits: usize,
    pub expected_cache_misses: usize,
    pub medians: Vec<MedianSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MedianSummary {
    pub engine: String,
    pub query: String,
    pub samples: usize,
    pub median_ns: u64,
}

pub fn execute(plan: RunPlan, reporter: &mut impl Reporter) -> anyhow::Result<RunSummary> {
    execute_with_final_check(plan, reporter, |_| Ok(()))
}

pub(crate) fn execute_with_final_check<R, F>(
    plan: RunPlan,
    reporter: &mut R,
    final_check: F,
) -> anyhow::Result<RunSummary>
where
    R: Reporter,
    F: FnOnce(&mut R) -> anyhow::Result<()>,
{
    reporter.status(&format!(
        "Creating result bundle {}",
        plan.output_dir.display()
    ))?;
    let mut bundle = RunBundle::create(plan)?;
    let result = execute_inner(&mut bundle, reporter)
        .and_then(|()| final_check(reporter))
        .and_then(|()| bundle.complete());
    match result {
        Ok(()) => {
            reporter.status(&format!("Benchmark complete: {}", bundle.root().display()))?;
            Ok(summarize(bundle.record(), bundle.root()))
        }
        Err(error) => {
            if let Err(flush_error) = bundle.fail(&error) {
                return Err(error).context(format!(
                    "also failed to record the error in {}: {flush_error:#}",
                    bundle.root().display()
                ));
            }
            reporter.status(&format!(
                "Benchmark failed; partial results remain at {}",
                bundle.root().display()
            ))?;
            Err(error).context(format!(
                "partial result bundle: {}",
                bundle.root().display()
            ))
        }
    }
}

fn execute_inner(bundle: &mut RunBundle, reporter: &mut impl Reporter) -> anyhow::Result<()> {
    let _run_lock = crate::run_lock::RunLock::acquire(reporter)?;
    let plan = bundle.record().plan.clone();
    let config_sha256 = plan
        .config
        .as_deref()
        .map(|path| hashing::file_with_progress(path, "Sirius config identity", reporter))
        .transpose()?;
    if let (Some(path), Some(sha256)) = (&plan.config, &config_sha256) {
        bundle.set_config_identity(ConfigIdentity {
            source_path: path.clone(),
            sha256: sha256.clone(),
        })?;
    }
    reporter.status("Capturing benchmark host information")?;
    let mut progress_error = None;
    let host = {
        let mut doctor_progress = |event: DoctorProgressEvent| {
            if progress_error.is_none() {
                progress_error = reporter.status(&event.message).err();
            }
        };
        capture_host_environment_system(&mut doctor_progress)
    };
    if let Some(error) = progress_error {
        return Err(error.into());
    }
    bundle.set_host(host)?;

    let prepared_dataset =
        dataset::prepare(&plan.dataset, &plan.repo_root, &plan.data_root, reporter)?;
    bundle.set_dataset(prepared_dataset.clone())?;

    let needs_sirius = plan.engines.contains(&Engine::Sirius);
    let prepared_build = if needs_sirius {
        Some(build_artifact::prepare(
            &plan.build,
            &plan.repo_root,
            plan.config.as_deref(),
            reporter,
        )?)
    } else {
        None
    };
    if let Some(build) = &prepared_build {
        ensure!(
            build.identity.config_sha256 == config_sha256,
            "Sirius configuration changed while preparing the build"
        );
        bundle.set_build(build.clone())?;
    }
    if !needs_sirius {
        reporter.status("Skipping the Sirius build for this DuckDB-only run")?;
    }
    let extension_snapshot = prepared_build
        .as_ref()
        .map(|build| FileSnapshot::capture(&build.extension))
        .transpose()?;

    let worker = Worker::materialize(bundle.root(), &plan.repo_root, reporter)?;
    let reference = worker.reference_identity(reporter)?;
    bundle.set_reference(reference.clone())?;
    let reference_module_snapshot = FileSnapshot::capture(&reference.module_path)?;
    let python_executable_snapshot = FileSnapshot::capture(&reference.python_executable)?;

    let expected = prepare_expected_results(
        bundle,
        &plan,
        &prepared_dataset,
        &reference,
        &worker,
        reporter,
    )?;
    ensure_reference_runtime_unchanged(
        &reference,
        &reference_module_snapshot,
        &python_executable_snapshot,
        "during expected-result preparation",
        reporter,
    )?;

    dataset::verify_stable(
        &plan.dataset,
        &prepared_dataset,
        "before measured execution",
        reporter,
    )?;
    reporter.status("All preparation is complete; starting measured trials")?;
    let trial_count = plan.engines.len() * plan.queries.len();
    let mut trial_index = 0_u32;
    for (query_index, query) in plan.queries.iter().enumerate() {
        let engines: Vec<Engine> = if query_index.is_multiple_of(2) {
            plan.engines.clone()
        } else {
            plan.engines.iter().copied().rev().collect()
        };
        for engine in engines {
            trial_index += 1;
            ensure_reference_runtime_unchanged(
                &reference,
                &reference_module_snapshot,
                &python_executable_snapshot,
                "before the trial",
                reporter,
            )?;
            if engine == Engine::Sirius {
                ensure_config_unchanged(
                    plan.config.as_deref(),
                    config_sha256.as_deref(),
                    "before the trial",
                    reporter,
                )?;
                let build = prepared_build
                    .as_ref()
                    .context("Sirius trial has no prepared build")?;
                extension_snapshot
                    .as_ref()
                    .context("Sirius trial has no extension snapshot")?
                    .ensure_unchanged(
                        &build.extension,
                        "Sirius extension",
                        "before the trial",
                        reporter,
                    )?;
            }
            reporter.status(&format!(
                "Starting trial {trial_index}/{trial_count}: {}/{}, fresh worker",
                engine_name(engine),
                query.name,
            ))?;
            let trial = worker.trial(
                engine,
                &query.name,
                &query.sql,
                plan.dataset.spec.format,
                &prepared_dataset.path,
                prepared_build
                    .as_ref()
                    .map(|build| build.extension.as_path()),
                plan.config.as_deref(),
                &reference,
                plan.pin,
                plan.execution.warmups,
                plan.execution.iterations,
                plan.execution.timeout_s,
                reporter,
            )?;
            dataset::verify_stable(
                &plan.dataset,
                &prepared_dataset,
                &format!("after the {}/{} trial", engine_name(engine), query.name),
                reporter,
            )?;
            ensure_reference_runtime_unchanged(
                &reference,
                &reference_module_snapshot,
                &python_executable_snapshot,
                "during the trial",
                reporter,
            )?;
            if engine == Engine::Sirius {
                ensure_config_unchanged(
                    plan.config.as_deref(),
                    config_sha256.as_deref(),
                    "during the trial",
                    reporter,
                )?;
                let build = prepared_build
                    .as_ref()
                    .context("Sirius trial has no prepared build")?;
                extension_snapshot
                    .as_ref()
                    .context("Sirius trial has no extension snapshot")?
                    .ensure_unchanged(
                        &build.extension,
                        "Sirius extension",
                        "during the trial",
                        reporter,
                    )?;
            }
            ensure!(
                trial.fallback_disabled == (engine == Engine::Sirius),
                "worker returned inconsistent fallback evidence"
            );
            for measurement in trial.measurements {
                let execution_class = if engine == Engine::Sirius {
                    "sirius_no_duckdb_fallback"
                } else {
                    "duckdb_cpu"
                };
                bundle.add_measurement(MeasurementRecord {
                    trial_index,
                    engine: trial.engine.clone(),
                    query: trial.query.clone(),
                    iteration: measurement.iteration,
                    duration_ns: measurement.duration_ns,
                    status: "ok".to_string(),
                    result_digest: measurement.result.digest.to_string(),
                    row_count: measurement.result.row_count,
                    execution_class: execution_class.to_string(),
                    pin_policy: trial.pin.clone(),
                    pin_setup_succeeded: trial.pin_setup_succeeded,
                })?;

                if let Some(expected) = expected.get(&query.name) {
                    reporter.status(&format!(
                        "Comparing {}/{} iteration {} with the expected result",
                        trial.engine, trial.query, measurement.iteration
                    ))?;
                    let outcome =
                        validation::compare(expected, &measurement.result, &query.validation);
                    if outcome.matched {
                        reporter.status(&format!(
                            "Validated {}/{} iteration {}",
                            trial.engine, trial.query, measurement.iteration
                        ))?;
                    } else {
                        reporter.status(&format!(
                            "Validation mismatch in {}/{} iteration {} ({} differences)",
                            trial.engine,
                            trial.query,
                            measurement.iteration,
                            outcome.mismatch_count
                        ))?;
                    }
                    bundle.add_validation(ValidationRecord {
                        engine: trial.engine.clone(),
                        query: trial.query.clone(),
                        iteration: measurement.iteration,
                        outcome,
                    })?;
                }
            }
        }
    }
    dataset::verify_stable(
        &plan.dataset,
        &prepared_dataset,
        "after measured execution",
        reporter,
    )?;
    if let Some(build) = &prepared_build {
        ensure_extension_digest_unchanged(build, "at run completion", reporter)?;
    }
    ensure_file_digest_unchanged(
        &reference.module_path,
        &reference.module_sha256,
        "DuckDB Python module",
        "at run completion",
        reporter,
    )?;
    ensure_file_digest_unchanged(
        &reference.python_executable,
        &reference.python_executable_sha256,
        "Python executable",
        "at run completion",
        reporter,
    )?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct FileSnapshot {
    bytes: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_seconds: i64,
    #[cfg(unix)]
    change_nanoseconds: i64,
}

impl FileSnapshot {
    fn capture(path: &Path) -> anyhow::Result<Self> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("reading metadata for {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                bytes: metadata.len(),
                modified: metadata.modified().ok(),
                device: metadata.dev(),
                inode: metadata.ino(),
                change_seconds: metadata.ctime(),
                change_nanoseconds: metadata.ctime_nsec(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                bytes: metadata.len(),
                modified: metadata.modified().ok(),
            })
        }
    }

    fn ensure_unchanged(
        &self,
        path: &Path,
        label: &str,
        phase: &str,
        reporter: &mut impl Reporter,
    ) -> anyhow::Result<()> {
        let current = Self::capture(path)?;
        ensure!(
            &current == self,
            "{label} {} changed {phase}; refusing to record mixed-artifact measurements",
            path.display()
        );
        reporter.detail(&format!("{label} metadata is unchanged {phase}"))?;
        Ok(())
    }
}

fn ensure_reference_runtime_unchanged(
    reference: &ReferenceIdentity,
    module_snapshot: &FileSnapshot,
    python_snapshot: &FileSnapshot,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    module_snapshot.ensure_unchanged(
        &reference.module_path,
        "DuckDB Python module",
        phase,
        reporter,
    )?;
    python_snapshot.ensure_unchanged(
        &reference.python_executable,
        "Python executable",
        phase,
        reporter,
    )
}

fn ensure_extension_digest_unchanged(
    build: &build_artifact::PreparedBuild,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    ensure_file_digest_unchanged(
        &build.extension,
        &build.identity.extension_sha256,
        "Sirius extension stability",
        phase,
        reporter,
    )
}

fn ensure_file_digest_unchanged(
    path: &Path,
    expected_sha256: &str,
    label: &str,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    let current = hashing::file_with_progress(path, label, reporter)?;
    ensure!(
        current == expected_sha256,
        "{label} {} changed {phase}; refusing to record mixed-runtime measurements",
        path.display()
    );
    reporter.detail(&format!(
        "{label} remained unchanged {phase} ({})",
        &current[..current.len().min(12)]
    ))?;
    Ok(())
}

fn ensure_config_unchanged(
    config: Option<&Path>,
    expected_sha256: Option<&str>,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    let (Some(config), Some(expected_sha256)) = (config, expected_sha256) else {
        return Ok(());
    };
    let current = hashing::file(config)?;
    ensure!(
        current == expected_sha256,
        "Sirius configuration {} changed {phase}; refusing to record incomparable measurements",
        config.display()
    );
    reporter.detail(&format!(
        "Sirius configuration remained unchanged {phase} ({})",
        &current[..current.len().min(12)]
    ))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_expected_results(
    bundle: &mut RunBundle,
    plan: &RunPlan,
    dataset: &dataset::PreparedDataset,
    reference_identity: &ReferenceIdentity,
    worker: &Worker,
    reporter: &mut impl Reporter,
) -> anyhow::Result<HashMap<String, ExpectedResult>> {
    if !plan.execution.validate {
        reporter.status("Validation is disabled for this benchmark")?;
        return Ok(HashMap::new());
    }

    reporter.status("Resolving expected results for selected queries")?;
    let cache = ExpectedCache::new(plan.data_root.join(".sirius/expected"));
    let reference = reference_artifact(reference_identity)?;
    let mut results = HashMap::new();
    for query in &plan.queries {
        let key = ExpectedCacheKey::new(
            DatasetReceiptId::new(dataset.identity.clone())?,
            query.sql.as_bytes(),
            reference.clone(),
            VALIDATION_PROTOCOL_VERSION,
        )?;
        reporter.status(&format!(
            "Checking the expected-result cache for {}",
            query.name
        ))?;
        let (entry, cache_hit) = match cache.lookup(&key)? {
            ExpectedCacheLookup::Hit(entry) => {
                reporter.status(&format!("Expected-result cache hit: {}", query.name))?;
                (entry, true)
            }
            ExpectedCacheLookup::Miss(miss) => {
                reporter.status(&format!(
                    "Expected-result cache miss: {}; claiming generation work",
                    query.name
                ))?;
                let reservation = cache.reserve(miss, |elapsed| {
                    reporter.status(&format!(
                        "Waiting for another process to generate {} ({})",
                        query.name,
                        crate::progress::format_duration(elapsed)
                    ))?;
                    Ok(())
                })?;
                let (entry, cache_hit) = match reservation {
                    ReservationOutcome::AlreadyPresent(entry) => (entry, true),
                    ReservationOutcome::Reserved(reservation) => {
                        reporter.status(&format!(
                            "Generating expected result for {} with DuckDB",
                            query.name
                        ))?;
                        dataset::verify_stable(
                            &plan.dataset,
                            dataset,
                            &format!("before reference generation for {}", query.name),
                            reporter,
                        )?;
                        let trial = worker.trial(
                            Engine::Duckdb,
                            &query.name,
                            &query.sql,
                            plan.dataset.spec.format,
                            &dataset.path,
                            None,
                            None,
                            reference_identity,
                            PinPolicy::None,
                            0,
                            1,
                            plan.execution.timeout_s,
                            reporter,
                        )?;
                        dataset::verify_stable(
                            &plan.dataset,
                            dataset,
                            &format!("after reference generation for {}", query.name),
                            reporter,
                        )?;
                        let generated = trial
                            .measurements
                            .into_iter()
                            .next()
                            .context("reference worker returned no result")?
                            .result;
                        let entry = reservation.publish_checked(generated, || {
                            dataset::verify_stable(
                                &plan.dataset,
                                dataset,
                                &format!(
                                    "immediately before publishing the expected result for {}",
                                    query.name
                                ),
                                reporter,
                            )
                        })?;
                        (entry, false)
                    }
                };
                reporter.status(&format!(
                    "Expected result ready: {} ({})",
                    query.name,
                    if cache_hit {
                        "another process populated the cache"
                    } else {
                        "published to cache"
                    }
                ))?;
                (entry, cache_hit)
            }
        };
        dataset::verify_stable(
            &plan.dataset,
            dataset,
            &format!("before recording the expected result for {}", query.name),
            reporter,
        )?;
        record_expected(bundle, &query.name, &entry, cache_hit)?;
        results.insert(query.name.clone(), entry.result);
    }
    reporter.status("All expected results are ready; no reference work remains in timed trials")?;
    Ok(results)
}

fn reference_artifact(identity: &ReferenceIdentity) -> anyhow::Result<ReferenceArtifact> {
    let settings = BTreeMap::from([
        (
            "duckdb_version".to_string(),
            ReferenceSettingValue::Text(identity.duckdb_version.clone()),
        ),
        (
            "threads".to_string(),
            ReferenceSettingValue::UnsignedInteger(identity.duckdb_threads.to_string()),
        ),
        (
            "preserve_insertion_order".to_string(),
            ReferenceSettingValue::Boolean(identity.preserve_insertion_order),
        ),
        (
            "python_version".to_string(),
            ReferenceSettingValue::Text(identity.python_version.clone()),
        ),
        (
            "python_executable_sha256".to_string(),
            ReferenceSettingValue::Text(identity.python_executable_sha256.clone()),
        ),
        (
            "timezone".to_string(),
            ReferenceSettingValue::Text("UTC".to_string()),
        ),
        (
            "worker_sha256".to_string(),
            ReferenceSettingValue::Text(identity.worker_sha256.clone()),
        ),
    ]);
    ReferenceArtifact::new(
        ReferenceArtifactId::new(identity.module_sha256.clone())?,
        settings,
    )
}

fn record_expected(
    bundle: &mut RunBundle,
    query: &str,
    entry: &CachedExpectedResult,
    cache_hit: bool,
) -> anyhow::Result<()> {
    bundle.add_expected(ExpectedResultRecord {
        query: query.to_string(),
        cache_id: entry.cache_id.to_string(),
        cache_hit,
        digest: entry.result.digest.to_string(),
        row_count: entry.result.row_count,
    })
}

pub(crate) fn summarize(record: &RunRecord, root: &Path) -> RunSummary {
    let validation_mismatches = record
        .validations
        .iter()
        .filter(|validation| !validation.outcome.matched)
        .count();
    let mut grouped: BTreeMap<(String, String), Vec<u64>> = BTreeMap::new();
    for measurement in &record.measurements {
        grouped
            .entry((measurement.engine.clone(), measurement.query.clone()))
            .or_default()
            .push(measurement.duration_ns);
    }
    let medians = grouped
        .into_iter()
        .map(|((engine, query), mut samples)| {
            samples.sort_unstable();
            let median_ns = median(&samples);
            MedianSummary {
                engine,
                query,
                samples: samples.len(),
                median_ns,
            }
        })
        .collect();
    RunSummary {
        status: "complete".to_string(),
        benchmark: record.plan.benchmark.name.clone(),
        bundle: root.to_path_buf(),
        measurement_count: record.measurements.len(),
        validation_status: record.validation_status,
        validation_mismatches,
        expected_cache_hits: record
            .expected_results
            .iter()
            .filter(|entry| entry.cache_hit)
            .count(),
        expected_cache_misses: record
            .expected_results
            .iter()
            .filter(|entry| !entry.cache_hit)
            .count(),
        medians,
    }
}

fn median(samples: &[u64]) -> u64 {
    match samples.len() {
        0 => 0,
        length if length % 2 == 1 => samples[length / 2],
        length => {
            let left = samples[length / 2 - 1];
            let right = samples[length / 2];
            left / 2 + right / 2 + (left % 2 + right % 2) / 2
        }
    }
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Sirius => "sirius",
        Engine::Duckdb => "duckdb",
        Engine::Both => "both",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_is_stable_for_even_and_odd_samples() {
        assert_eq!(median(&[]), 0);
        assert_eq!(median(&[3]), 3);
        assert_eq!(median(&[1, 3]), 2);
        assert_eq!(median(&[1, 2, 100]), 2);
    }

    #[test]
    fn reference_cache_identity_includes_explicit_duckdb_execution_settings() {
        let identity = ReferenceIdentity {
            duckdb_version: "1.5.4".to_owned(),
            duckdb_threads: 12,
            preserve_insertion_order: true,
            module_path: "/runtime/_duckdb.so".into(),
            module_sha256: "a".repeat(64),
            python_version: "3.14.0".to_owned(),
            python_executable: "/runtime/python".into(),
            python_executable_sha256: "b".repeat(64),
            worker_sha256: "c".repeat(64),
        };

        let artifact = reference_artifact(&identity).unwrap();

        assert_eq!(
            artifact.settings.get("threads"),
            Some(&ReferenceSettingValue::UnsignedInteger("12".to_owned()))
        );
        assert_eq!(
            artifact.settings.get("preserve_insertion_order"),
            Some(&ReferenceSettingValue::Boolean(true))
        );
    }

    #[test]
    fn config_mutation_is_detected_before_results_are_recorded() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("sirius.yaml");
        std::fs::write(&config, "original").unwrap();
        let expected = hashing::file(&config).unwrap();
        std::fs::write(&config, "changed").unwrap();
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);

        let error = ensure_config_unchanged(
            Some(&config),
            Some(&expected),
            "during the trial",
            &mut progress,
        )
        .unwrap_err();

        assert!(error.to_string().contains("changed during the trial"));
    }
}
