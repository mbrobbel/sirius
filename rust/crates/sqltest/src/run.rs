use crate::{
    config::{AxisOverride, Config, Name, PathBinding, Selection},
    corpus::{self, Expected, Step},
    report::{Baseline, CaseReport, Outcome, Provenance, Report},
    result,
    worker::{Operation, Response, Worker},
};
use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Args)]
pub struct RunArgs {
    #[arg(long, default_value = "test/sqltest")]
    pub root: PathBuf,
    #[arg(long, default_value = ".cache/sqltest/data")]
    pub fixtures: PathBuf,
    #[arg(long)]
    pub run: Option<Name>,
    #[arg(long)]
    pub suite: Option<Selection>,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub select: Option<String>,
    #[arg(long, value_name = "NAME=CHOICE[,CHOICE]")]
    pub axis: Vec<AxisOverride>,
    #[arg(long, value_name = "NAME=PATH")]
    pub suite_dir: Vec<PathBinding>,
    #[arg(long)]
    pub extension: Option<PathBuf>,
    #[arg(long, conflicts_with = "extension")]
    pub cpu_only: bool,
    #[arg(long, default_value = "runs/sqltest")]
    pub output: PathBuf,
    #[arg(long)]
    pub previous: Option<PathBuf>,
    #[arg(long, default_value = "unknown")]
    pub runtime_revision: String,
    #[arg(long)]
    pub source_run: Option<String>,
    #[arg(long)]
    pub source_commit: Option<String>,
}

pub fn runtime_path() -> Result<PathBuf> {
    let maps = fs::read_to_string("/proc/self/maps")?;
    let path = maps
        .lines()
        .filter_map(|s| s.split_whitespace().last())
        .find(|s| s.contains("/libduckdb.so"))
        .context("cannot locate dynamically loaded libduckdb.so")?;
    Ok(PathBuf::from(path))
}

fn revision() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
        .unwrap_or_else(|| "unknown".into())
}

fn devices() -> (usize, String) {
    match Command::new("nvidia-smi")
        .args(["--query-gpu=name,driver_version", "--format=csv,noheader"])
        .output()
    {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let visible = std::env::var("CUDA_VISIBLE_DEVICES").ok();
            let count = visible.as_ref().map_or(text.lines().count(), |s| {
                if s.is_empty() || s == "-1" {
                    0
                } else {
                    s.split(',').count()
                }
            });
            (
                count,
                format!(
                    "{text}; CUDA_VISIBLE_DEVICES={}",
                    visible.as_deref().unwrap_or("all")
                ),
            )
        }
        _ => (0, "unavailable".into()),
    }
}

enum CaseSelector<'a> {
    Id(&'a str),
    File(&'a str),
    Location { file: &'a str, line: u32 },
}

impl<'a> CaseSelector<'a> {
    fn parse(value: &'a str, ids: &std::collections::BTreeSet<String>) -> Result<Self> {
        if ids.contains(value) {
            Ok(Self::Id(value))
        } else if let Some((file, line)) = value.rsplit_once(':') {
            Ok(Self::Location {
                file,
                line: line.parse().context("selector line must be an integer")?,
            })
        } else {
            Ok(Self::File(value))
        }
    }
}

fn selected(case: &corpus::Case, selector: Option<&CaseSelector<'_>>) -> bool {
    selector.is_none_or(|selector| match selector {
        CaseSelector::Id(id) => case.id == *id,
        CaseSelector::File(file) => case.file.contains(file),
        CaseSelector::Location { file, line } => case.line == *line && case.file.ends_with(file),
    })
}

pub fn assert_statement(response: Response, expected: &Expected) -> Result<()> {
    match (response, expected) {
        (Response::Statement { .. }, Expected::Ok) => Ok(()),
        (Response::Statement { count }, Expected::Count(expected)) if count == *expected => Ok(()),
        (
            Response::Error {
                message,
                harness: false,
            },
            Expected::Error(pattern),
        ) if regex::Regex::new(pattern)?.is_match(message.trim()) => Ok(()),
        (response, expected) => bail!("expected {expected:?}, received {response:?}"),
    }
}

pub fn execute(args: &RunArgs) -> Result<bool> {
    let config = Config::load_with_suites(&args.root, &args.suite_dir)?;
    let root = &config.root;
    let fixtures = std::path::absolute(&args.fixtures)?;
    let plan = config.plan(args.run.as_ref(), args.suite.as_ref(), &args.axis)?;
    let mut scripts = std::collections::BTreeMap::new();
    let mut ids = std::collections::BTreeSet::new();
    for target in &plan.targets {
        if scripts.contains_key(&target.suite) {
            continue;
        }
        let suite = &config.suites[&target.suite];
        let loaded = corpus::discover(root, &suite.directory)?;
        let context = crate::substitutions::ContextValues {
            fixtures: &fixtures,
            relation: config.manifest.storage[&target.storage].relation,
            scratch: Path::new("validation"),
            bucket: suite
                .object_store
                .as_ref()
                .map(|name| config.manifest.services[name].bucket()),
        };
        for script in &loaded {
            for step in &script.steps {
                for engine in [corpus::Engine::DuckDb, corpus::Engine::Sirius] {
                    let sql = match step {
                        Step::Setup { sql, scope, .. } if scope.applies(engine.clone()) => {
                            Some(sql)
                        }
                        Step::Query(case) => Some(&case.sql),
                        _ => None,
                    };
                    if let Some(sql) = sql {
                        context
                            .sql(sql, &suite.substitutions, engine)
                            .with_context(|| {
                                format!("substitutions in {}", script.path.display())
                            })?;
                    }
                }
                if let Step::Query(case) = step {
                    ensure!(ids.insert(case.id.clone()), "duplicate case ID {}", case.id);
                }
            }
        }
        scripts.insert(target.suite.clone(), loaded);
    }
    let selector = args
        .select
        .as_deref()
        .map(|value| CaseSelector::parse(value, &ids))
        .transpose()?;
    let selected_count = plan.targets.iter().map(|target| scripts[&target.suite].iter().map(|script| script.steps.iter().filter(|step| matches!(step, Step::Query(case) if selected(case, selector.as_ref()))).count()).sum::<usize>()).sum::<usize>();
    ensure!(selected_count > 0, "selection matched no queries");
    if args.dry_run {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"plan": plan, "cases": selected_count})
            )?
        );
        return Ok(true);
    }
    ensure!(
        args.cpu_only || args.extension.is_some(),
        "provide --extension or explicitly use --cpu-only"
    );
    let baseline = Baseline::load(&root.join("gaps.toml"))?;
    ensure!(
        !args.output.exists() || fs::read_dir(&args.output)?.next().is_none(),
        "output directory is not empty; choose a new --output"
    );
    fs::create_dir_all(&args.output)?;
    let output = args.output.canonicalize()?;
    fs::write(output.join("plan.json"), serde_json::to_vec_pretty(&plan)?)?;
    fs::write(
        output.join("sqltest.toml"),
        toml::to_string_pretty(&config.manifest)?,
    )?;
    fs::write(
        output.join("suites.json"),
        serde_json::to_vec_pretty(&config.suites)?,
    )?;
    let extension = args
        .extension
        .as_ref()
        .map(|p| p.canonicalize())
        .transpose()?;
    let (gpu_count, device) = if args.cpu_only {
        (0, "cpu-only".into())
    } else {
        devices()
    };
    let runtime = runtime_path()?;
    let con = crate::worker::connect(None, false)?;
    let version: String = con.query_row("SELECT version()", [], |r| r.get(0))?;
    drop(con);
    let mut report = Report::new(Provenance {
        extension_sha256: extension
            .as_ref()
            .map(|p| corpus::file_hash(p))
            .transpose()?,
        runtime_sha256: corpus::file_hash(&runtime)?,
        runtime_revision: args.runtime_revision.clone(),
        duckdb_version: version,
        runner_revision: revision(),
        runner_sha256: corpus::file_hash(&std::env::current_exe()?)?,
        suite_revision: revision(),
        source_run: args.source_run.clone(),
        source_commit: args.source_commit.clone(),
        device,
        cpu_only: args.cpu_only,
    });
    if args.cpu_only {
        report.execution_evidence =
            "CPU-only harness validation; this run measures no Sirius coverage".into();
    }
    let previous: Option<Report> = args
        .previous
        .as_ref()
        .map(|p| -> Result<_> { Ok(serde_json::from_slice(&fs::read(p)?)?) })
        .transpose()?;
    let fixture_hashes: std::collections::BTreeMap<_, _> = scripts
        .keys()
        .map(|name| {
            (
                name.clone(),
                crate::fixtures::verify(&config, &fixtures, &config.suites[name].fixtures),
            )
        })
        .collect();
    report.save(&output, previous.as_ref())?;
    for target in &plan.targets {
        let suite = &config.suites[&target.suite];
        let checkpoint = suite.checkpoint;
        let profile = &config.manifest.profiles[&target.profile];
        let configuration = target.label();
        let configuration_id = corpus::hash(serde_json::to_vec(&(
            &target.profile,
            &target.storage,
            &target.axes,
            &target.settings,
            &target.reference_settings,
        ))?);
        let storage = &config.manifest.storage[&target.storage];
        let profile_config = config.path(&profile.config)?;
        let service_spec = suite
            .object_store
            .as_ref()
            .map(|name| &config.manifest.services[name]);
        let fixture_hash = &fixture_hashes[&target.suite];
        for script in &scripts[&target.suite] {
            let Some(last_selected) = script.steps.iter().rposition(
                |step| matches!(step, Step::Query(case) if selected(case, selector.as_ref())),
            ) else {
                continue;
            };
            let identity = corpus::hash(format!("{}:{configuration_id}", script.path.display()));
            let work = output.join("work").join(&identity[..16]);
            fs::create_dir_all(&work)?;
            let mut abort: Option<String> = None;
            let mut abort_outcome = Outcome::InfrastructureFailure;
            let unavailable = !args.cpu_only && profile.gpus.get() > gpu_count;
            if unavailable {
                abort = Some(format!(
                    "profile needs {} GPUs, found {gpu_count}",
                    profile.gpus
                ));
            }
            if let Err(error) = fixture_hash {
                abort = Some(format!("fixture unavailable: {error:#}"));
            }
            let mut service = None;
            let mut worker_profile = profile_config.clone();
            let pair = if abort.is_none() {
                (|| -> Result<_> {
                    if let Some(spec) = service_spec.filter(|_| !args.cpu_only) {
                        service = Some(spec.start(&fixtures)?);
                        let resolved = work.join("sirius.yaml");
                        service
                            .as_ref()
                            .unwrap()
                            .write_profile(&profile_config, &resolved)?;
                        worker_profile = resolved;
                    }
                    for worker in ["reference", "sirius"] {
                        crate::fixtures::stage(
                            &config,
                            &suite.script_setup(),
                            &fixtures,
                            &work.join(worker),
                        )?;
                    }
                    let reference = Worker::start(
                        &work.join("reference"),
                        None,
                        None,
                        &Default::default(),
                        checkpoint,
                        &target.reference_settings,
                    )?;
                    let actual = Worker::start(
                        &work.join("sirius"),
                        extension.as_deref(),
                        Some(&worker_profile),
                        &profile.environment,
                        checkpoint,
                        &target.settings,
                    )?;
                    Ok((reference, actual))
                })()
            } else {
                Err(anyhow::anyhow!("requirements unavailable"))
            };
            let mut workers = match pair {
                Ok((reference, actual)) => {
                    ensure!(
                        reference.version == report.provenance.duckdb_version
                            && actual.version == reference.version,
                        "worker runtime mismatch"
                    );
                    Some((reference, actual))
                }
                Err(e) => {
                    if abort.is_none() {
                        abort = Some(format!("{e:#}"));
                    }
                    None
                }
            };
            let reference_work = work.join("reference");
            let actual_work = work.join("sirius");
            let reference_context = crate::substitutions::ContextValues {
                fixtures: &fixtures,
                relation: storage.relation,
                scratch: &reference_work,
                bucket: service_spec.map(crate::services::Service::bucket),
            };
            let actual_context = crate::substitutions::ContextValues {
                scratch: &actual_work,
                ..reference_context
            };
            let actual_engine = if args.cpu_only {
                corpus::Engine::DuckDb
            } else {
                corpus::Engine::Sirius
            };
            let mut setup_sql = String::new();
            let mut reference_sql = String::new();
            let steps = if args.select.is_some() {
                &script.steps[..=last_selected]
            } else {
                &script.steps
            };
            for (index, step) in steps.iter().enumerate() {
                match step {
                    Step::Setup {
                        connection,
                        sql,
                        expected,
                        scope,
                    } => {
                        let reference_applies = scope.applies(corpus::Engine::DuckDb);
                        let actual_applies = scope.applies(actual_engine.clone());
                        let reference_statement = if reference_applies {
                            reference_context.sql(
                                sql,
                                &suite.substitutions,
                                corpus::Engine::DuckDb,
                            )?
                        } else {
                            String::new()
                        };
                        let sql = if actual_applies {
                            actual_context.sql(sql, &suite.substitutions, actual_engine.clone())?
                        } else {
                            String::new()
                        };
                        if reference_applies {
                            reference_sql.push_str(&Operation::Statement.reproduction(
                                &reference_statement,
                                false,
                                checkpoint,
                            ));
                        }
                        if actual_applies {
                            setup_sql.push_str(&Operation::Statement.reproduction(
                                &sql,
                                extension.is_some(),
                                checkpoint,
                            ));
                        }
                        if let Some((reference, actual)) = workers.as_mut() {
                            let setup = (|| -> Result<()> {
                                if reference_applies {
                                    assert_statement(
                                        reference.execute(
                                            &reference_statement,
                                            connection,
                                            Operation::Statement,
                                            &work.join("unused.arrow"),
                                            300,
                                        )?,
                                        expected,
                                    )
                                    .context("reference setup")?;
                                }
                                if actual_applies {
                                    assert_statement(
                                        actual.execute(
                                            &sql,
                                            connection,
                                            Operation::Statement,
                                            &work.join("unused.arrow"),
                                            300,
                                        )?,
                                        expected,
                                    )
                                    .context("Sirius setup")?;
                                }
                                Ok(())
                            })();
                            if let Err(e) = setup {
                                abort = Some(format!("{e:#}"));
                                workers = None;
                                if index > last_selected {
                                    let item =
                                        report.cases.last_mut().expect("last query was reported");
                                    item.outcome = Outcome::InfrastructureFailure;
                                    item.message = format!("setup after final query failed: {e:#}");
                                    item.expected_gap = None;
                                    item.unexpected_pass = false;
                                    let directory = output.join(&item.artifacts);
                                    if !script.has_named_connections() {
                                        fs::write(directory.join("repro.sql"), &setup_sql)?;
                                        fs::write(
                                            directory.join("reference-repro.sql"),
                                            &reference_sql,
                                        )?;
                                    }
                                    fs::write(
                                        output.join(&item.artifacts).join("result.json"),
                                        serde_json::to_vec_pretty(item)?,
                                    )?;
                                    report.save(&output, previous.as_ref())?;
                                }
                            }
                        }
                    }
                    Step::Query(case) => {
                        let is_selected = selected(case, selector.as_ref());
                        let category = if is_selected {
                            "cases"
                        } else {
                            "prerequisites"
                        };
                        let relative = format!("{category}/{configuration_id}/{}", case.id);
                        let directory = output.join(&relative);
                        fs::create_dir_all(&directory)?;
                        let reference_query_sql = reference_context.sql(
                            &case.sql,
                            &suite.substitutions,
                            corpus::Engine::DuckDb,
                        )?;
                        let sql = actual_context.sql(
                            &case.sql,
                            &suite.substitutions,
                            actual_engine.clone(),
                        )?;
                        let actual_query = Operation::Query(case.execution).reproduction(
                            &sql,
                            extension.is_some(),
                            checkpoint,
                        );
                        let reference_query = Operation::Query(corpus::Execution::NoFallback)
                            .reproduction(&reference_query_sql, false, checkpoint);
                        fs::write(directory.join("repro.slt"), &script.expanded)?;
                        suite.write_reproduction(&directory.join("suite.toml"))?;
                        if !script.has_named_connections() {
                            fs::write(
                                directory.join("repro.sql"),
                                format!("{setup_sql}\n{actual_query}"),
                            )?;
                            fs::write(
                                directory.join("reference-repro.sql"),
                                format!("{reference_sql}\n{reference_query}"),
                            )?;
                        }
                        fs::copy(&worker_profile, directory.join("sirius.yaml"))?;
                        if let Some(spec) = service_spec {
                            fs::copy(&profile_config, directory.join("sirius-template.yaml"))?;
                            fs::write(
                                directory.join("service.json"),
                                serde_json::to_vec_pretty(&serde_json::json!({
                                    "recipe": spec,
                                    "runtime": service.as_ref().map(|service| &service.metadata),
                                }))?,
                            )?;
                        }
                        fs::write(
                            directory.join("environment.json"),
                            serde_json::to_vec_pretty(&profile.environment)?,
                        )?;
                        fs::write(
                            directory.join("configuration.json"),
                            serde_json::to_vec_pretty(target)?,
                        )?;
                        fs::write(
                            directory.join("sirius-settings.sql"),
                            target.settings.sql().join("\n"),
                        )?;
                        fs::write(
                            directory.join("reference-settings.sql"),
                            target.reference_settings.sql().join("\n"),
                        )?;
                        fs::write(
                            directory.join("case.json"),
                            serde_json::to_vec_pretty(case)?,
                        )?;
                        let fingerprint = corpus::hash(format!(
                            "{}:{}:{}:{}",
                            script.fingerprint,
                            fixture_hash
                                .as_ref()
                                .map(String::as_str)
                                .unwrap_or("unavailable"),
                            corpus::file_hash(&profile_config)?,
                            serde_json::to_string(&(
                                storage,
                                checkpoint,
                                &profile.environment,
                                &suite.scratch_directories,
                                &suite.fixture_files,
                                &suite.substitutions,
                                service_spec,
                                &target.settings,
                                &target.reference_settings,
                                &case.tolerances,
                                &case.float_tolerance
                            ))?
                        ));
                        let mut item = CaseReport::new(case, target, fingerprint, relative);
                        if let Some(message) = &abort {
                            item.outcome = if unavailable {
                                Outcome::UnmetRequirement
                            } else {
                                abort_outcome
                            };
                            item.message = message.clone();
                            abort_outcome = Outcome::Blocked;
                        } else if let Some((reference, actual)) = workers.as_mut() {
                            evaluate(
                                case,
                                &reference_query_sql,
                                &sql,
                                &directory,
                                reference,
                                actual,
                                &mut item,
                            );
                            if matches!(
                                item.outcome,
                                Outcome::Crash
                                    | Outcome::Timeout
                                    | Outcome::ReferenceFailure
                                    | Outcome::HarnessError
                            ) || (!is_selected && item.outcome != Outcome::Match)
                            {
                                abort =
                                    Some(format!("previous case {}: {}", case.id, item.message));
                                abort_outcome = Outcome::Blocked;
                                workers = None;
                            }
                        }
                        if is_selected && !args.cpu_only {
                            baseline.apply(&mut item);
                        }
                        eprintln!(
                            "{} [{configuration}]: {:?}{}{}",
                            case.id,
                            item.outcome,
                            if item.expected_gap.is_some() {
                                " (known gap)"
                            } else {
                                ""
                            },
                            if is_selected { "" } else { " (prerequisite)" }
                        );
                        fs::write(
                            directory.join("result.json"),
                            serde_json::to_vec_pretty(&item)?,
                        )?;
                        fs::write(
                            directory.join("worker-logs.txt"),
                            format!("{}\n", work.strip_prefix(&output)?.display()),
                        )?;
                        setup_sql.push_str(&actual_query);
                        reference_sql.push_str(&reference_query);
                        if is_selected {
                            report.cases.push(item);
                            report.save(&output, previous.as_ref())?;
                        }
                    }
                }
            }
            drop(workers);
            if let Some(service) = &service
                && let Err(error) = service.save_logs(&work)
            {
                eprintln!("cannot save MinIO logs: {error:#}");
            }
        }
    }
    ensure!(
        !report.cases.is_empty(),
        "selection has no cases supporting the requested storage"
    );
    report.complete = report.cases.iter().all(|c| {
        !matches!(
            c.outcome,
            Outcome::Blocked
                | Outcome::UnmetRequirement
                | Outcome::ReferenceFailure
                | Outcome::InfrastructureFailure
                | Outcome::HarnessError
        )
    });
    report.save(&output, previous.as_ref())?;
    println!(
        "{} / {} cases match DuckDB; {} expected gaps; complete: {}",
        report
            .cases
            .iter()
            .filter(|c| c.outcome == Outcome::Match)
            .count(),
        report.cases.len(),
        report
            .cases
            .iter()
            .filter(|c| c.expected_gap.is_some())
            .count(),
        report.complete
    );
    println!("Report: {}", output.join("summary.md").display());
    Ok(!report.failed())
}

fn evaluate(
    case: &corpus::Case,
    reference_sql: &str,
    actual_sql: &str,
    directory: &Path,
    reference: &mut Worker,
    actual: &mut Worker,
    item: &mut CaseReport,
) {
    let reference_path = directory.join("reference.arrow");
    let actual_path = directory.join("sirius.arrow");
    let reference_response = match reference.execute(
        reference_sql,
        &case.connection,
        Operation::Query(corpus::Execution::NoFallback),
        &reference_path,
        case.timeout,
    ) {
        Ok(r) => r,
        Err(e) => {
            item.outcome = Outcome::ReferenceFailure;
            item.message = format!("{e:#}");
            return;
        }
    };
    match &reference_response {
        Response::Error { message, harness } => {
            if *harness
                || !matches!(&case.expected, Expected::Error(pattern) if regex::Regex::new(pattern).unwrap().is_match(message.trim()))
            {
                item.outcome = Outcome::ReferenceFailure;
                item.message = message.clone();
                return;
            }
        }
        Response::Rows { .. } => {
            if matches!(case.expected, Expected::Error(_)) {
                item.outcome = Outcome::ReferenceFailure;
                item.message = "reference unexpectedly succeeded".into();
                return;
            }
            let validation = (|| -> Result<()> {
                let batch = result::read(&reference_path)?;
                item.reference_rows = Some(batch.num_rows());
                let snapshot = result::snapshot(&batch, case.ordered)?;
                fs::write(directory.join("reference.txt"), snapshot.join("\n"))?;
                if case.snapshot {
                    ensure!(
                        batch.num_columns() == case.expected_columns
                            && snapshot == case.expected_rows,
                        "committed snapshot differs from DuckDB; review using complete"
                    );
                }
                result::validate_alignment(&batch, case.ordered, &case.resolved_tolerances(&batch))
            })();
            if let Err(e) = validation {
                item.outcome = if e.is::<result::Limitation>() {
                    Outcome::HarnessError
                } else {
                    Outcome::ReferenceFailure
                };
                item.message = format!("{e:#}");
                return;
            }
        }
        _ => {
            item.outcome = Outcome::HarnessError;
            item.message = "invalid reference response".into();
            return;
        }
    }
    let actual_response = match actual.execute(
        actual_sql,
        &case.connection,
        Operation::Query(case.execution),
        &actual_path,
        case.timeout,
    ) {
        Ok(r) => r,
        Err(e) => {
            item.outcome = if e.chain().any(|e| {
                e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    )
                })
            }) {
                Outcome::Timeout
            } else {
                Outcome::Crash
            };
            item.message = format!("{e:#}");
            return;
        }
    };
    match (reference_response, actual_response) {
        (
            _,
            Response::Error {
                message,
                harness: true,
            },
        ) => {
            item.outcome = Outcome::HarnessError;
            item.message = message;
        }
        (
            Response::Error { .. },
            Response::Error {
                message,
                harness: false,
            },
        ) if matches!(&case.expected, Expected::Error(pattern) if regex::Regex::new(pattern).unwrap().is_match(message.trim())) => {
            item.outcome = Outcome::Match
        }
        (_, Response::Error { message, .. }) => {
            item.outcome = Outcome::Error;
            item.message = message;
        }
        (
            Response::Rows {
                logical_types: reference_types,
            },
            Response::Rows {
                logical_types: actual_types,
            },
        ) => {
            let comparison = (|| -> Result<()> {
                let reference = result::read(&reference_path)?;
                let actual = result::read(&actual_path)?;
                item.reference_rows = Some(reference.num_rows());
                fs::write(
                    directory.join("reference.txt"),
                    result::snapshot(&reference, case.ordered)?.join("\n"),
                )?;
                fs::write(
                    directory.join("sirius.txt"),
                    result::snapshot(&actual, case.ordered)?.join("\n"),
                )?;
                ensure!(
                    reference_types == actual_types,
                    "logical types differ: {reference_types:?} != {actual_types:?}"
                );
                result::compare(
                    &reference,
                    &actual,
                    case.ordered,
                    &case.resolved_tolerances(&reference),
                )
            })();
            match comparison {
                Ok(()) => item.outcome = Outcome::Match,
                Err(e) => {
                    item.outcome =
                        if e.is::<result::Limitation>() || e.is::<arrow::error::ArrowError>() {
                            Outcome::HarnessError
                        } else {
                            Outcome::Mismatch
                        };
                    item.message = format!("{e:#}");
                }
            }
        }
        _ => {
            item.outcome = Outcome::Mismatch;
            item.message = "expected error but Sirius succeeded".into();
        }
    }
}
