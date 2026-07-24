use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    build_artifact::PreparedBuild,
    dataset::PreparedDataset,
    doctor::HostEnvironment,
    model::{RUN_RESULT_SCHEMA_VERSION, ReferenceIdentity, RunPlan},
    validation::ValidationOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    Pending,
    Disabled,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub schema_version: u32,
    pub status: RunStatus,
    pub started_unix_ms: u128,
    pub finished_unix_ms: Option<u128>,
    pub plan: RunPlan,
    pub host: Option<HostEnvironment>,
    pub config_identity: Option<ConfigIdentity>,
    pub dataset: Option<PreparedDataset>,
    pub build: Option<PreparedBuild>,
    pub reference: Option<ReferenceIdentity>,
    pub expected_results: Vec<ExpectedResultRecord>,
    pub measurements: Vec<MeasurementRecord>,
    pub validations: Vec<ValidationRecord>,
    pub validation_status: ValidationStatus,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigIdentity {
    pub source_path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedResultRecord {
    pub query: String,
    pub cache_id: String,
    pub cache_hit: bool,
    pub digest: String,
    pub row_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRecord {
    pub trial_index: u32,
    pub engine: String,
    pub query: String,
    pub iteration: u32,
    pub duration_ns: u64,
    pub status: String,
    pub result_digest: String,
    pub row_count: u64,
    pub execution_class: String,
    pub pin_policy: String,
    pub pin_setup_succeeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationRecord {
    pub engine: String,
    pub query: String,
    pub iteration: u32,
    pub outcome: ValidationOutcome,
}

pub struct RunBundle {
    root: PathBuf,
    record: RunRecord,
}

impl RunBundle {
    pub fn create(plan: RunPlan) -> anyhow::Result<Self> {
        ensure!(
            !plan.output_dir.exists(),
            "result bundle {} already exists",
            plan.output_dir.display()
        );
        if let Some(parent) = plan.output_dir.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::create_dir(&plan.output_dir)
            .with_context(|| format!("creating result bundle {}", plan.output_dir.display()))?;
        fs::create_dir(plan.output_dir.join("logs"))
            .with_context(|| format!("creating logs in {}", plan.output_dir.display()))?;
        let validation_status = if plan.execution.validate {
            ValidationStatus::Pending
        } else {
            ValidationStatus::Disabled
        };
        let mut bundle = Self {
            root: plan.output_dir.clone(),
            record: RunRecord {
                schema_version: RUN_RESULT_SCHEMA_VERSION,
                status: RunStatus::Running,
                started_unix_ms: unix_millis(),
                finished_unix_ms: None,
                plan,
                host: None,
                config_identity: None,
                dataset: None,
                build: None,
                reference: None,
                expected_results: Vec::new(),
                measurements: Vec::new(),
                validations: Vec::new(),
                validation_status,
                error: None,
            },
        };
        bundle.flush()?;
        Ok(bundle)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn record(&self) -> &RunRecord {
        &self.record
    }

    pub fn set_dataset(&mut self, dataset: PreparedDataset) -> anyhow::Result<()> {
        self.record.dataset = Some(dataset);
        self.flush()
    }

    pub fn set_build(&mut self, build: PreparedBuild) -> anyhow::Result<()> {
        self.record.build = Some(build);
        self.flush()
    }

    pub fn set_reference(&mut self, reference: ReferenceIdentity) -> anyhow::Result<()> {
        self.record.reference = Some(reference);
        self.flush()
    }

    pub fn set_host(&mut self, host: HostEnvironment) -> anyhow::Result<()> {
        self.record.host = Some(host);
        self.flush()
    }

    pub fn set_config_identity(&mut self, config: ConfigIdentity) -> anyhow::Result<()> {
        self.record.config_identity = Some(config);
        self.flush()
    }

    pub fn add_expected(&mut self, expected: ExpectedResultRecord) -> anyhow::Result<()> {
        self.record.expected_results.push(expected);
        self.flush()
    }

    pub fn add_measurement(&mut self, measurement: MeasurementRecord) -> anyhow::Result<()> {
        self.record.measurements.push(measurement);
        self.flush()
    }

    pub fn add_validation(&mut self, validation: ValidationRecord) -> anyhow::Result<()> {
        self.record.validations.push(validation);
        self.flush()
    }

    pub fn complete(&mut self) -> anyhow::Result<()> {
        self.record.status = RunStatus::Complete;
        self.record.finished_unix_ms = Some(unix_millis());
        if self.record.validation_status != ValidationStatus::Disabled {
            self.record.validation_status = if self.record.validations.len()
                == self.record.measurements.len()
                && self
                    .record
                    .validations
                    .iter()
                    .all(|validation| validation.outcome.matched)
            {
                ValidationStatus::Passed
            } else {
                ValidationStatus::Failed
            };
        }
        self.flush()
    }

    pub fn fail(&mut self, error: &anyhow::Error) -> anyhow::Result<()> {
        self.record.status = RunStatus::Failed;
        self.record.finished_unix_ms = Some(unix_millis());
        self.record.error = Some(format!("{error:#}"));
        self.flush()
    }

    fn flush(&mut self) -> anyhow::Result<()> {
        atomic_json(&self.root.join("run.json"), &self.record)?;
        atomic_csv(&self.root.join("runtimes.csv"), &self.record.measurements)
    }
}

fn atomic_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    let temporary = temporary_path(path);
    let output =
        File::create(&temporary).with_context(|| format!("creating {}", temporary.display()))?;
    serde_json::to_writer_pretty(&output, value)
        .with_context(|| format!("writing {}", temporary.display()))?;
    output.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("publishing {}", path.display()))
}

fn atomic_csv(path: &Path, measurements: &[MeasurementRecord]) -> anyhow::Result<()> {
    let temporary = temporary_path(path);
    let mut output =
        File::create(&temporary).with_context(|| format!("creating {}", temporary.display()))?;
    writeln!(
        output,
        "trial_index,engine,query,iteration,duration_ns,status,result_digest,row_count,execution_class,pin_policy,pin_setup_succeeded"
    )?;
    for measurement in measurements {
        writeln!(
            output,
            "{},{},{},{},{},{},{},{},{},{},{}",
            measurement.trial_index,
            csv_field(&measurement.engine),
            csv_field(&measurement.query),
            measurement.iteration,
            measurement.duration_ns,
            csv_field(&measurement.status),
            csv_field(&measurement.result_digest),
            measurement.row_count,
            csv_field(&measurement.execution_class),
            csv_field(&measurement.pin_policy),
            measurement.pin_setup_succeeded,
        )?;
    }
    output.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("publishing {}", path.display()))
}

fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!("tmp-{}", std::process::id()))
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use crate::{
        assets::{DataFormat, Engine},
        model::{
            BenchmarkPlan, BuildAction, BuildPlan, CacheState, DatasetPlan, DatasetSpec,
            ExecutionOrigin, ExecutionPlan, PLAN_SCHEMA_VERSION, PinPolicy,
        },
    };

    use super::*;

    fn plan(root: &Path) -> RunPlan {
        let data = root.join("data");
        RunPlan {
            schema_version: PLAN_SCHEMA_VERSION,
            runner_version: "test".into(),
            runner_sha256: "0".repeat(64),
            origin: ExecutionOrigin::Local,
            repository: crate::model::RepositoryIdentity {
                git_commit: Some("test".into()),
                git_dirty: Some(false),
            },
            benchmark: BenchmarkPlan {
                name: "test".into(),
                description: None,
                suite: "tpch".into(),
            },
            repo_root: root.into(),
            data_root: data.clone(),
            output_dir: root.join("run"),
            dataset: DatasetPlan {
                spec: DatasetSpec {
                    schema_version: 1,
                    kind: "tpch".into(),
                    generator: "test".into(),
                    generator_version: 1,
                    scale_factor: 1.0,
                    format: DataFormat::Parquet,
                },
                recipe: Some(crate::model::DatasetRecipe {
                    schema_version: 1,
                    wrapper_sha256: "0".repeat(64),
                    tpchgen_revision: "0".repeat(40),
                    jobs: 8,
                }),
                id: "dataset".into(),
                entry_dir: data.clone(),
                data_path: data,
                cache: CacheState::Miss,
                estimated_bytes: 1,
                managed: true,
                verify_content: false,
            },
            build: BuildPlan {
                preset: Some("release".into()),
                build_dir: root.join("build"),
                duckdb_binary: root.join("build/duckdb"),
                extension: root.join("build/sirius.duckdb_extension"),
                action: BuildAction::IncrementalBuild,
            },
            queries: Vec::new(),
            engines: vec![Engine::Duckdb],
            execution: ExecutionPlan {
                warmups: 1,
                iterations: 1,
                timeout_s: 10,
                validate: true,
                cache_state: "test".into(),
                timing_boundary: "test".into(),
                trial_order: "test".into(),
            },
            config: None,
            pin: PinPolicy::None,
            sources: Default::default(),
        }
    }

    #[test]
    fn bundle_is_visible_as_running_then_atomically_completed() {
        let temp = tempfile::tempdir().unwrap();
        let mut bundle = RunBundle::create(plan(temp.path())).unwrap();
        let running: RunRecord =
            serde_json::from_reader(File::open(bundle.root().join("run.json")).unwrap()).unwrap();
        assert_eq!(running.status, RunStatus::Running);
        bundle.complete().unwrap();
        let complete: RunRecord =
            serde_json::from_reader(File::open(bundle.root().join("run.json")).unwrap()).unwrap();
        assert_eq!(complete.status, RunStatus::Complete);
        assert_eq!(complete.validation_status, ValidationStatus::Passed);
    }

    #[test]
    fn disabled_validation_is_never_reported_as_passed() {
        let temp = tempfile::tempdir().unwrap();
        let mut plan = plan(temp.path());
        plan.execution.validate = false;
        let mut bundle = RunBundle::create(plan).unwrap();
        bundle.complete().unwrap();

        assert_eq!(
            bundle.record().validation_status,
            ValidationStatus::Disabled
        );
    }

    #[test]
    fn config_identity_is_recorded_without_copying_secret_bearing_contents() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.yaml");
        let mut bundle = RunBundle::create(plan(temp.path())).unwrap();
        bundle
            .set_config_identity(ConfigIdentity {
                source_path: source.clone(),
                sha256: "a".repeat(64),
            })
            .unwrap();

        let recorded = bundle.record().config_identity.as_ref().unwrap();
        assert_eq!(recorded.source_path, source);
        assert_eq!(recorded.sha256, "a".repeat(64));
        assert!(!bundle.root().join("inputs/sirius.yaml").exists());
    }
}
