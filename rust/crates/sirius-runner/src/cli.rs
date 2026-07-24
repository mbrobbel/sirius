use std::{
    io::{self, Write},
    num::NonZeroU32,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    assets::{Assets, BenchManifest, DatasetManifest, Engine, SuiteManifest},
    doctor::{
        DiagnosticStatus, DoctorProgressEvent, LocalDoctorRequest,
        diagnose_local_system_for_workload, diagnose_remote_with_repository_for_workload,
    },
    model::{BuildAction, CacheState, PinPolicy, RunPlan},
    plan::{self, RunOverrides},
    progress::Progress,
    remote::{ProgressEvent, RemoteRunId, SshTarget, SystemCommandRunner},
    remote_execution::{
        CudaProfile, RemoteClientOutcome, RemoteClientRequest, RemoteExecutionClient,
        RemoteInvocation, run_hidden_worker,
    },
    run_bundle::ValidationStatus,
    runner::RunSummary,
};

/// Run repeatable Sirius benchmarks locally and in CI.
#[derive(Debug, Parser)]
#[command(version, propagate_version = true)]
#[command(after_help = "\
Examples:
  sirius-runner list
  sirius-runner show tpch-sf1
  sirius-runner run tpch-sf1 --dry-run
  sirius-runner run tpch-sf1 --queries q1,q6 --iterations 5
  sirius-runner doctor

Docs & issues: https://github.com/sirius-db/sirius (rust/crates/sirius-runner)")]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn run(self) -> anyhow::Result<CommandOutcome> {
        let stdout = io::stdout();
        let mut progress = Progress::stderr(self.globals.verbose);
        self.run_with(&mut stdout.lock(), &mut progress)
    }

    fn run_with<Out: Write, ProgressOut: Write>(
        self,
        stdout: &mut Out,
        progress: &mut Progress<ProgressOut>,
    ) -> anyhow::Result<CommandOutcome> {
        let assets = Assets::resolve(None);
        match self.command {
            Command::List => {
                progress.status("Loading benchmark definitions")?;
                progress.detail("Using definitions embedded in sirius-runner")?;
                self.globals.write_names(stdout, assets.bench_names()?)?;
                Ok(CommandOutcome::Success)
            }
            Command::Show { name } => {
                progress.status(format!("Loading benchmark `{name}`"))?;
                let benchmark = assets.load_bench(&name)?;
                let suite = assets.load_suite(&benchmark.bench.suite)?;
                let dataset = assets.load_dataset(&suite.suite.dataset)?;
                self.globals.write_benchmark_details(
                    stdout,
                    &BenchmarkDetails {
                        benchmark,
                        suite,
                        dataset,
                    },
                )?;
                Ok(CommandOutcome::Success)
            }
            Command::Run(args) => {
                if let Some(target) = args.remote_options.remote.clone() {
                    anyhow::ensure!(
                        self.globals.data_root.is_none(),
                        "--data-root applies only to local runs; use --remote-data-root with --remote"
                    );
                    let remote_repo = args
                        .remote_options
                        .remote_repo
                        .clone()
                        .expect("clap requires --remote-repo with --remote");
                    return dispatch_remote_run(
                        args,
                        target,
                        remote_repo,
                        &self.globals,
                        stdout,
                        progress,
                    );
                }

                progress.status(format!("Resolving benchmark `{}`", args.name))?;
                let dry_run = args.dry_run;
                let plan = plan::resolve(&assets, args.into_overrides(&self.globals))?;
                if dry_run {
                    progress.status("Dry run complete; no changes were made")?;
                    self.globals.write_plan(stdout, &plan)?;
                    Ok(CommandOutcome::Success)
                } else {
                    let summary = crate::runner::execute(plan, progress)?;
                    self.globals.write_run_summary(stdout, &summary)?;
                    if summary.validation_status == ValidationStatus::Failed {
                        Ok(CommandOutcome::ValidationMismatch)
                    } else {
                        Ok(CommandOutcome::Success)
                    }
                }
            }
            Command::RemoteWorker { action } => {
                run_hidden_worker(&action)?;
                Ok(CommandOutcome::Success)
            }
            Command::Doctor(args) => {
                let cuda_required = args.engine.unwrap_or(Engine::Both) != Engine::Duckdb;
                let allow_source_difference = args.remote_options.allow_remote_source_difference;
                if let Some(target) = args.remote_options.remote {
                    anyhow::ensure!(
                        self.globals.data_root.is_none(),
                        "--data-root applies only to local diagnostics"
                    );
                    let remote_repo = args
                        .remote_options
                        .remote_repo
                        .expect("clap requires --remote-repo with --remote");
                    progress.status(format!("Checking remote environment {}", target.as_str()))?;
                    progress.detail(format!("Remote repository: {}", remote_repo.display()))?;
                    let local_repo = plan::resolve_repo_root(self.globals.repo_root.as_deref())?;
                    progress.detail(format!("Local repository: {}", local_repo.display()))?;
                    let mut progress_error = None;
                    let report = {
                        let mut doctor_progress = |event: DoctorProgressEvent| {
                            if progress_error.is_none() {
                                progress_error = progress.status(event.message).err();
                            }
                        };
                        diagnose_remote_with_repository_for_workload(
                            &target,
                            &local_repo,
                            &remote_repo,
                            cuda_required,
                            allow_source_difference,
                            &mut SystemCommandRunner,
                            &mut doctor_progress,
                        )
                    };
                    if let Some(error) = progress_error {
                        return Err(error.into());
                    }
                    self.globals.write_doctor_report(stdout, &report)?;
                    Ok(if report.status == DiagnosticStatus::Blocked {
                        CommandOutcome::PrerequisitesBlocked
                    } else {
                        CommandOutcome::Success
                    })
                } else {
                    progress.status("Checking the local Sirius environment")?;
                    let repo_root = match self.globals.repo_root.clone() {
                        Some(path) => path,
                        None => plan::resolve_repo_root(None)?,
                    };
                    let data_root = self
                        .globals
                        .data_root
                        .clone()
                        .or_else(|| std::env::var_os("SIRIUS_RUNNER_DATA_ROOT").map(PathBuf::from))
                        .unwrap_or_else(|| PathBuf::from("test_datasets"));
                    let request = LocalDoctorRequest::new(repo_root, data_root);
                    let mut progress_error = None;
                    let report = {
                        let mut doctor_progress = |event: DoctorProgressEvent| {
                            if progress_error.is_none() {
                                progress_error = progress.status(event.message).err();
                            }
                        };
                        diagnose_local_system_for_workload(
                            &request,
                            cuda_required,
                            &mut doctor_progress,
                        )
                    };
                    if let Some(error) = progress_error {
                        return Err(error.into());
                    }
                    self.globals.write_doctor_report(stdout, &report)?;
                    Ok(if report.status == DiagnosticStatus::Blocked {
                        CommandOutcome::PrerequisitesBlocked
                    } else {
                        CommandOutcome::Success
                    })
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    Success,
    PrerequisitesBlocked,
    ValidationMismatch,
    ExecutionFailed,
}

impl CommandOutcome {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::PrerequisitesBlocked => 1,
            Self::ValidationMismatch => 3,
            Self::ExecutionFailed => 1,
        }
    }
}

pub fn is_broken_pipe(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .and_then(serde_json::Error::io_error_kind)
                == Some(io::ErrorKind::BrokenPipe)
    })
}

#[derive(Debug, Args)]
#[command(next_help_heading = "Global options")]
pub struct GlobalArgs {
    /// Local Sirius checkout root; defaults to strict repository discovery.
    #[arg(long, global = true, value_name = "DIR")]
    pub repo_root: Option<PathBuf>,

    /// Root for locally managed datasets; defaults under the repository.
    #[arg(long, global = true, value_name = "DIR")]
    pub data_root: Option<PathBuf>,

    /// Emit one machine-readable JSON document on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    /// Show additional progress details on stderr.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl GlobalArgs {
    fn write_names(&self, writer: &mut impl Write, names: Vec<String>) -> anyhow::Result<()> {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, &names)?;
            writeln!(writer)?;
        } else {
            for name in names {
                writeln!(writer, "{name}")?;
            }
        }
        Ok(())
    }

    fn write_benchmark_details(
        &self,
        writer: &mut impl Write,
        details: &BenchmarkDetails,
    ) -> anyhow::Result<()> {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, details)?;
            writeln!(writer)?;
            return Ok(());
        }

        writeln!(writer, "Name: {}", details.benchmark.bench.name)?;
        if let Some(description) = &details.benchmark.bench.description {
            writeln!(writer, "Description: {description}")?;
        }
        writeln!(writer, "Suite: {}", details.suite.suite.name)?;
        writeln!(
            writer,
            "Dataset: {} SF{} ({})",
            details.dataset.dataset.name,
            details.benchmark.dataset.scale_factor,
            data_format_name(details.benchmark.dataset.format),
        )?;
        writeln!(
            writer,
            "Engine: {}",
            engine_name(details.benchmark.engine.engine)
        )?;
        writeln!(
            writer,
            "Queries: {}",
            details
                .suite
                .queries
                .iter()
                .map(|query| query.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(writer, "Warm-ups: {}", details.benchmark.execution.warmups)?;
        writeln!(
            writer,
            "Measured iterations: {}",
            details.benchmark.execution.iterations
        )?;
        writeln!(
            writer,
            "Validation: {} via {}",
            if details.benchmark.execution.validate {
                "enabled"
            } else {
                "disabled"
            },
            details.suite.validation.reference
        )?;
        Ok(())
    }

    fn write_plan(&self, writer: &mut impl Write, plan: &RunPlan) -> anyhow::Result<()> {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, plan)?;
            writeln!(writer)?;
            return Ok(());
        }

        writeln!(writer, "Benchmark: {}", plan.benchmark.name)?;
        writeln!(writer, "Repository: {}", plan.repo_root.display())?;
        writeln!(
            writer,
            "Dataset: {} ({}, {})",
            plan.dataset.data_path.display(),
            data_format_name(plan.dataset.spec.format),
            cache_state_name(plan.dataset.cache)
        )?;
        writeln!(
            writer,
            "Dataset verification: {}",
            if plan.dataset.verify_content {
                "full content hashes"
            } else {
                "size and modification time"
            }
        )?;
        writeln!(
            writer,
            "Build: {} ({})",
            plan.build.build_dir.display(),
            build_action_name(plan.build.action)
        )?;
        writeln!(
            writer,
            "Engines: {}",
            plan.engines
                .iter()
                .map(|engine| engine_name(*engine))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(
            writer,
            "Queries: {}",
            plan.queries
                .iter()
                .map(|query| query.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )?;
        writeln!(writer, "Warm-ups: {}", plan.execution.warmups)?;
        writeln!(writer, "Measured iterations: {}", plan.execution.iterations)?;
        writeln!(writer, "Timeout: {}s", plan.execution.timeout_s)?;
        writeln!(writer, "Timing: {}", plan.execution.timing_boundary)?;
        writeln!(writer, "Trial order: {}", plan.execution.trial_order)?;
        writeln!(writer, "Cache state: {}", plan.execution.cache_state)?;
        writeln!(writer, "Validate: {}", plan.execution.validate)?;
        writeln!(writer, "Pin: {}", pin_name(plan.pin))?;
        match &plan.config {
            Some(path) => writeln!(writer, "Config: {}", path.display())?,
            None => writeln!(writer, "Config: engine default")?,
        }
        writeln!(writer, "Output: {}", plan.output_dir.display())?;
        Ok(())
    }

    fn write_run_summary(
        &self,
        writer: &mut impl Write,
        summary: &RunSummary,
    ) -> anyhow::Result<()> {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, summary)?;
            writeln!(writer)?;
            return Ok(());
        }

        writeln!(writer, "Benchmark: {}", summary.benchmark)?;
        writeln!(writer, "Status: {}", summary.status)?;
        writeln!(writer, "Bundle: {}", summary.bundle.display())?;
        writeln!(writer, "Measurements: {}", summary.measurement_count)?;
        writeln!(
            writer,
            "Validation: {} ({} mismatches)",
            validation_status_name(summary.validation_status),
            summary.validation_mismatches
        )?;
        writeln!(
            writer,
            "Expected-result cache: {} hits, {} misses",
            summary.expected_cache_hits, summary.expected_cache_misses
        )?;
        for median in &summary.medians {
            writeln!(
                writer,
                "{} {}: {} ns median ({} samples)",
                median.engine, median.query, median.median_ns, median.samples
            )?;
        }
        Ok(())
    }

    fn write_remote_outcome(
        &self,
        writer: &mut impl Write,
        outcome: &RemoteClientOutcome,
    ) -> anyhow::Result<()> {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, outcome)?;
            writeln!(writer)?;
            return Ok(());
        }

        match outcome {
            RemoteClientOutcome::DryRun(dry_run) => {
                writeln!(writer, "Remote dry run: ready")?;
                writeln!(
                    writer,
                    "Remote: {} (run {})",
                    dry_run.target, dry_run.run_id
                )?;
                writeln!(
                    writer,
                    "Target: {} / {} / {}",
                    dry_run.compatibility.os,
                    dry_run.compatibility.architecture,
                    cuda_profile_name(dry_run.execution_target.cuda_profile)
                )?;
                writeln!(
                    writer,
                    "Remote repository: {}",
                    dry_run.remote_repo.display()
                )?;
                write_remote_repository_state(
                    writer,
                    "Remote revision",
                    &dry_run.remote_repository,
                )?;
                if let Some(local) = &dry_run.local_repository {
                    write_remote_repository_state(writer, "Local revision", local)?;
                    writeln!(writer, "Source parity: enforced")?;
                } else {
                    writeln!(writer, "Source parity: explicit override")?;
                }
                writeln!(
                    writer,
                    "Pixi environment: {} ({})",
                    dry_run.pack_key,
                    if dry_run.remote_pack_cache_hit {
                        "remote cache hit"
                    } else if dry_run.local_pack_cache_hit == Some(true) {
                        "local cache hit; upload required"
                    } else if dry_run.local_pack_cache_hit.is_none() {
                        "local cache check skipped"
                    } else {
                        "cache miss; pack required"
                    }
                )?;
                if let Some(version) = &dry_run.local_pixi_version {
                    writeln!(writer, "Local Pixi: {version}")?;
                }
                writeln!(writer, "Planned actions:")?;
                for action in &dry_run.planned_actions {
                    writeln!(writer, "- {action}")?;
                }
            }
            RemoteClientOutcome::Executed(executed) => {
                writeln!(writer, "Remote status: {:?}", executed.status)?;
                writeln!(
                    writer,
                    "Remote: {} (run {})",
                    executed.target, executed.run_id
                )?;
                writeln!(
                    writer,
                    "Remote repository: {}",
                    executed.remote_repo.display()
                )?;
                write_remote_repository_state(
                    writer,
                    "Remote revision",
                    &executed.remote_repository,
                )?;
                if let Some(local) = &executed.local_repository {
                    write_remote_repository_state(writer, "Local revision", local)?;
                    writeln!(writer, "Source parity: enforced")?;
                } else {
                    writeln!(writer, "Source parity: explicit override")?;
                }
                writeln!(
                    writer,
                    "Target: {} / {} / {}",
                    executed.compatibility.os,
                    executed.compatibility.architecture,
                    cuda_profile_name(executed.execution_target.cuda_profile)
                )?;
                writeln!(
                    writer,
                    "Pixi environment: {} ({})",
                    executed.pack_key,
                    if executed.remote_pack_cache_hit {
                        "remote cache hit"
                    } else {
                        "uploaded"
                    }
                )?;
                if let Some(summary) = &executed.summary {
                    self.write_run_summary(writer, summary)?;
                } else {
                    if let Some(output) = &executed.output {
                        writeln!(writer, "Bundle: {}", output.display())?;
                    }
                    if let Some(status) = executed.validation_status {
                        writeln!(writer, "Validation: {}", validation_status_name(status))?;
                    }
                }
                if let Some(failure) = &executed.failure {
                    writeln!(
                        writer,
                        "Remote failure: {}: {}",
                        failure.code, failure.message
                    )?;
                }
                if executed.remote_job_retained {
                    writeln!(
                        writer,
                        "Remote job: retained as {} for inspection",
                        executed.run_id
                    )?;
                } else {
                    writeln!(writer, "Remote job: removed after verified download")?;
                }
                if let Some(warning) = &executed.cleanup_warning {
                    writeln!(writer, "Remote cleanup warning: {warning}")?;
                }
            }
        }
        Ok(())
    }

    fn write_doctor_report<T>(&self, writer: &mut impl Write, report: &T) -> anyhow::Result<()>
    where
        T: Serialize + std::fmt::Display,
    {
        if self.json {
            serde_json::to_writer_pretty(&mut *writer, report)?;
            writeln!(writer)?;
        } else {
            write!(writer, "{report}")?;
        }
        Ok(())
    }
}

fn write_remote_repository_state(
    writer: &mut impl Write,
    label: &str,
    state: &crate::remote::RemoteRepositoryState,
) -> io::Result<()> {
    writeln!(
        writer,
        "{label}: {} ({})",
        &state.git_commit[..state.git_commit.len().min(12)],
        if state.git_dirty { "dirty" } else { "clean" }
    )
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkDetails {
    benchmark: BenchManifest,
    suite: SuiteManifest,
    dataset: DatasetManifest,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List available named benchmarks.
    List,

    /// Show a benchmark definition.
    Show {
        /// Benchmark name, for example tpch-sf1.
        name: String,
    },

    /// Prepare and run a named benchmark.
    Run(RunArgs),

    /// Check local or remote prerequisites and report actionable problems.
    Doctor(DoctorArgs),

    #[command(name = "__remote-worker", hide = true)]
    RemoteWorker {
        #[arg(value_parser = ["handshake", "run"])]
        action: String,
    },
}

/// Arguments that change a named benchmark run.
///
/// Execution-target options, including SSH transport, belong here when remote
/// execution is added. They must not become global behavior for inspection
/// commands.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Benchmark name, for example tpch-sf1.
    pub name: String,

    /// Run only these queries, for example q1,q6.
    #[arg(long, value_delimiter = ',', value_name = "QUERY")]
    pub queries: Vec<String>,

    /// Measured iterations per query.
    #[arg(long, value_name = "N")]
    pub iterations: Option<NonZeroU32>,

    /// Engine or engines to run.
    #[arg(long, value_enum)]
    pub engine: Option<Engine>,

    /// Sirius configuration on the selected host, relative to its checkout.
    #[arg(long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Sirius cache tier to pin for each query.
    #[arg(long, value_enum, default_value = "none")]
    pub pin: PinPolicy,

    /// Sirius build preset on the selected host; defaults to release.
    #[arg(long, conflicts_with = "build_dir")]
    pub preset: Option<String>,

    /// Existing build directory on the selected host, relative to its checkout.
    #[arg(long, value_name = "DIR")]
    pub build_dir: Option<PathBuf>,

    /// Existing dataset on the selected host, relative to its checkout.
    #[arg(long, value_name = "PATH")]
    pub data: Option<PathBuf>,

    /// Rehash all dataset files instead of trusting size and modification time.
    #[arg(long)]
    pub verify_data: bool,

    /// Exact local result-bundle directory; it must not already exist.
    #[arg(long, value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Perform read-only validation and print the plan or remote actions.
    #[arg(long)]
    pub dry_run: bool,

    #[command(flatten)]
    pub remote_options: RemoteTargetArgs,

    /// Remote dataset root; defaults to <remote-repo>/test_datasets.
    #[arg(
        long,
        requires = "remote",
        value_name = "DIR",
        value_parser = parse_absolute_path
    )]
    pub remote_data_root: Option<PathBuf>,
}

impl RunArgs {
    fn into_overrides(self, globals: &GlobalArgs) -> RunOverrides {
        RunOverrides {
            name: self.name,
            repo_root: globals.repo_root.clone(),
            data_root: globals.data_root.clone(),
            queries: self.queries,
            iterations: self.iterations.map(NonZeroU32::get),
            engine: self.engine,
            config: self.config,
            pin: self.pin,
            preset: self.preset,
            build_dir: self.build_dir,
            data: self.data,
            verify_data: self.verify_data,
            output: self.output,
        }
    }
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub remote_options: RemoteTargetArgs,

    /// Workload whose local or remote prerequisites should be checked.
    #[arg(long, value_enum)]
    pub engine: Option<Engine>,
}

#[derive(Debug, Args)]
pub struct RemoteTargetArgs {
    /// SSH destination to use.
    #[arg(
        long,
        requires = "remote_repo",
        value_name = "USER@HOST",
        value_parser = parse_ssh_target
    )]
    pub remote: Option<SshTarget>,

    /// Sirius checkout root on the remote machine.
    #[arg(
        long = "remote-repo",
        requires = "remote",
        value_name = "DIR",
        value_parser = parse_absolute_path
    )]
    pub remote_repo: Option<PathBuf>,

    /// Allow dirty or different local and remote source revisions.
    #[arg(long, requires = "remote")]
    pub allow_remote_source_difference: bool,
}

fn parse_ssh_target(value: &str) -> Result<SshTarget, String> {
    SshTarget::new(value).map_err(|error| error.to_string())
}

fn parse_absolute_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err("must be an absolute path".to_string())
    }
}

fn dispatch_remote_run(
    args: RunArgs,
    target: SshTarget,
    remote_repo: PathBuf,
    globals: &GlobalArgs,
    stdout: &mut impl Write,
    progress: &mut Progress<impl Write>,
) -> anyhow::Result<CommandOutcome> {
    progress.status(format!(
        "Preparing remote benchmark `{}` on {}",
        args.name,
        target.as_str()
    ))?;
    let local_repo = plan::resolve_repo_root(globals.repo_root.as_deref())?;
    let local_output = plan::resolve_output_dir(&local_repo, &args.name, args.output.as_deref())?;
    let remote_data_root = args
        .remote_data_root
        .clone()
        .unwrap_or_else(|| remote_repo.join("test_datasets"));
    let mut invocation = RemoteInvocation::new(&args.name, remote_repo, remote_data_root);
    invocation.queries = args.queries;
    invocation.iterations = args.iterations.map(NonZeroU32::get);
    invocation.engine = args.engine;
    invocation.config = args.config;
    invocation.pin = args.pin;
    invocation.preset = args.preset;
    invocation.build_dir = args.build_dir;
    invocation.data = args.data;
    invocation.verify_data = args.verify_data;
    invocation.allow_source_difference = args.remote_options.allow_remote_source_difference;
    let request = RemoteClientRequest {
        target,
        local_repo,
        local_output,
        run_id: new_remote_run_id()?,
        invocation,
    };

    let mut client = RemoteExecutionClient::current()?;
    let mut progress_error = None;
    let mut remote_progress = |event: ProgressEvent| {
        if progress_error.is_none() {
            progress_error = progress.status(event.message).err();
        }
    };
    let outcome = client.run(&request, args.dry_run, &mut remote_progress);
    if let Some(error) = progress_error {
        return Err(error.into());
    }
    let outcome = outcome?;
    globals.write_remote_outcome(stdout, &outcome)?;
    Ok(match outcome {
        RemoteClientOutcome::DryRun(_) => CommandOutcome::Success,
        RemoteClientOutcome::Executed(ref executed) if !executed.succeeded() => {
            CommandOutcome::ExecutionFailed
        }
        RemoteClientOutcome::Executed(ref executed)
            if executed.validation_status == Some(ValidationStatus::Failed) =>
        {
            CommandOutcome::ValidationMismatch
        }
        RemoteClientOutcome::Executed(_) => CommandOutcome::Success,
    })
}

fn new_remote_run_id() -> anyhow::Result<RemoteRunId> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    RemoteRunId::new(format!("run-{nanos}-{}", std::process::id()))
}

fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::Sirius => "sirius",
        Engine::Duckdb => "duckdb",
        Engine::Both => "both",
    }
}

fn cuda_profile_name(profile: CudaProfile) -> &'static str {
    match profile {
        CudaProfile::Auto => "auto",
        CudaProfile::Cuda12 => "cuda12",
        CudaProfile::Cuda13 => "cuda13",
    }
}

fn validation_status_name(status: ValidationStatus) -> &'static str {
    match status {
        ValidationStatus::Pending => "pending",
        ValidationStatus::Disabled => "disabled",
        ValidationStatus::Passed => "passed",
        ValidationStatus::Failed => "failed",
    }
}

fn data_format_name(format: crate::assets::DataFormat) -> &'static str {
    match format {
        crate::assets::DataFormat::Parquet => "parquet",
        crate::assets::DataFormat::Duckdb => "duckdb",
    }
}

fn cache_state_name(state: CacheState) -> &'static str {
    match state {
        CacheState::Hit => "cache hit",
        CacheState::Miss => "cache miss",
        CacheState::Invalid => "invalid cache entry",
        CacheState::External => "external",
        CacheState::Unknown => "unknown",
    }
}

fn build_action_name(action: BuildAction) -> &'static str {
    match action {
        BuildAction::IncrementalBuild => "incremental build",
        BuildAction::UseExisting => "existing build",
        BuildAction::NotRequired => "not required",
    }
}

fn pin_name(pin: PinPolicy) -> &'static str {
    match pin {
        PinPolicy::None => "none",
        PinPolicy::Gpu => "gpu",
        PinPolicy::Host => "host",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        path::Path,
    };

    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn command_outcomes_have_stable_exit_codes() {
        assert_eq!(CommandOutcome::Success.exit_code(), 0);
        assert_eq!(CommandOutcome::PrerequisitesBlocked.exit_code(), 1);
        assert_eq!(CommandOutcome::ValidationMismatch.exit_code(), 3);
    }

    #[test]
    fn public_surface_contains_only_v0_commands() {
        let help = Cli::command().render_long_help().to_string();

        for command in ["list", "show", "run", "doctor"] {
            assert!(help.contains(&format!("  {command}")));
        }
        for deferred in [
            "build",
            "dataset",
            "suite",
            "validate",
            "results",
            "compare",
            "sweep",
            "telemetry",
            "remote",
            "specs",
        ] {
            assert!(!help.contains(&format!("  {deferred}")));
        }
    }

    #[test]
    fn run_requires_a_named_benchmark_and_positive_iterations() {
        assert!(Cli::try_parse_from(["sirius-runner", "run"]).is_err());
        assert!(
            Cli::try_parse_from(["sirius-runner", "run", "tpch-sf1", "--iterations", "0"]).is_err()
        );
    }

    #[test]
    fn run_accepts_v0_overrides_and_global_json_after_subcommand() {
        let cli = Cli::try_parse_from([
            "sirius-runner",
            "run",
            "tpch-sf1",
            "--queries",
            "q1,q6",
            "--iterations",
            "5",
            "--engine",
            "both",
            "--pin",
            "gpu",
            "--dry-run",
            "--json",
        ])
        .unwrap();

        assert!(cli.globals.json);
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(run.name, "tpch-sf1");
        assert_eq!(run.queries, ["q1", "q6"]);
        assert_eq!(run.iterations.unwrap().get(), 5);
        assert!(run.dry_run);
        assert_eq!(run.pin, PinPolicy::Gpu);
    }

    #[test]
    fn remote_run_options_enforce_the_ssh_target_relationships() {
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "run",
                "tpch-sf1",
                "--remote",
                "developer@example.test",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "run",
                "tpch-sf1",
                "--remote-repo",
                "/srv/sirius",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "run",
                "tpch-sf1",
                "--remote",
                "developer@example.test",
                "--remote-repo",
                "relative/checkout",
            ])
            .is_err()
        );

        let cli = Cli::try_parse_from([
            "sirius-runner",
            "run",
            "tpch-sf1",
            "--remote",
            "developer@example.test",
            "--remote-repo",
            "/srv/sirius",
            "--remote-data-root",
            "/datasets",
        ])
        .unwrap();
        let Command::Run(run) = cli.command else {
            panic!("expected run command");
        };
        assert_eq!(
            run.remote_options.remote.unwrap().as_str(),
            "developer@example.test"
        );
        assert_eq!(
            run.remote_options.remote_repo.unwrap(),
            PathBuf::from("/srv/sirius")
        );
        assert_eq!(run.remote_data_root.unwrap(), PathBuf::from("/datasets"));
    }

    #[test]
    fn doctor_accepts_only_a_complete_remote_target() {
        assert!(Cli::try_parse_from(["sirius-runner", "doctor", "--engine", "duckdb"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "doctor",
                "--remote",
                "developer@example.test",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "doctor",
                "--remote",
                "invalid target",
                "--remote-repo",
                "/srv/sirius",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "sirius-runner",
                "doctor",
                "--remote",
                "developer@example.test",
                "--remote-repo",
                "/srv/sirius",
            ])
            .is_ok()
        );
    }

    #[test]
    fn json_results_and_progress_use_separate_streams() {
        let cli = Cli::try_parse_from(["sirius-runner", "list", "--json"]).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut progress = Progress::with_writer(&mut stderr, 0);

        cli.run_with(&mut stdout, &mut progress).unwrap();

        let names: Vec<String> = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(names, ["tpch-sf1", "tpch-sf100"]);
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "Loading benchmark definitions\n"
        );
    }

    #[test]
    fn show_includes_the_suite_dataset_and_query_inventory() {
        let cli = Cli::try_parse_from(["sirius-runner", "show", "tpch-sf1", "--json"]).unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut progress = Progress::with_writer(&mut stderr, 0);

        cli.run_with(&mut stdout, &mut progress).unwrap();

        let value: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(value["benchmark"]["bench"]["name"], "tpch-sf1");
        assert_eq!(value["suite"]["suite"]["name"], "tpch");
        assert_eq!(value["dataset"]["dataset"]["name"], "tpch");
        assert_eq!(value["suite"]["queries"].as_array().unwrap().len(), 22);
    }

    #[test]
    fn local_dry_run_outputs_the_resolved_plan_without_writes() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let data_root = temporary.path().join("datasets");
        let output = temporary.path().join("result");
        let cli = Cli::try_parse_from([
            "sirius-runner",
            "run",
            "tpch-sf1",
            "--repo-root",
            repo_root.to_str().unwrap(),
            "--data-root",
            data_root.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--queries",
            "q1,q6",
            "--dry-run",
            "--json",
        ])
        .unwrap();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut progress = Progress::with_writer(&mut stderr, 0);

        cli.run_with(&mut stdout, &mut progress).unwrap();

        let plan: RunPlan = serde_json::from_slice(&stdout).unwrap();
        assert_eq!(plan.benchmark.name, "tpch-sf1");
        assert_eq!(
            plan.queries
                .iter()
                .map(|query| query.name.as_str())
                .collect::<Vec<_>>(),
            ["q1", "q6"]
        );
        assert_eq!(plan.output_dir, output);
        assert!(!data_root.exists());
        assert!(!output.exists());
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "Resolving benchmark `tpch-sf1`\nDry run complete; no changes were made\n"
        );
    }

    #[test]
    fn json_output_reports_a_closed_pipe_without_panicking() {
        struct ClosedPipe;

        impl Write for ClosedPipe {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let cli = Cli::try_parse_from(["sirius-runner", "list", "--json"]).unwrap();
        let mut stderr = Vec::new();
        let mut progress = Progress::with_writer(&mut stderr, 0);

        let error = cli.run_with(&mut ClosedPipe, &mut progress).unwrap_err();

        assert!(is_broken_pipe(&error));
    }
}
