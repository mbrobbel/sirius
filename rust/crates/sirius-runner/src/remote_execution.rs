//! Benchmark-specific remote execution built on the generic SSH transport.

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    assets::{Assets, Engine},
    model::{ExecutionOrigin, PinPolicy},
    plan::{self, RunOverrides},
    progress::{Progress, Reporter},
    remote::{
        ArtifactDescriptor, CommandRunner, HandshakeRequest, HandshakeResponse, PackCacheKey,
        PixiPackCache, PixiPackFormat, PixiPackRequest, ProgressSink, REMOTE_PROTOCOL_VERSION,
        REMOTE_RESULT_VERSION, RemoteCompatibility, RemoteExecutor, RemoteFailure, RemotePlan,
        RemoteRunId, RemoteRunOutcome, RemoteRunRequest, RemoteRunStatus, RemoteStage,
        ResultEnvelope, SshTarget, SystemCommandRunner,
    },
    run_bundle::{RunRecord, RunStatus, ValidationStatus},
    runner,
};

pub const REMOTE_INVOCATION_VERSION: u32 = 5;
pub const PIXI_PACK_VERSION: &str = "0.7.10";

const CUDA_12_ARCHITECTURES: &str = "75-real;80-real;86-real;89-real;90a-real";
const CUDA_13_ARCHITECTURES: &str =
    "75-real;80-real;86-real;89-real;90a-real;100f-real;120a-real;120";
const CUDA_12_TOOLKIT: (u32, u32) = (12, 9);
const CUDA_13_TOOLKIT: (u32, u32) = (13, 2);
const WORKER_JOB_ENV: &str = "SIRIUS_REMOTE_JOB_DIR";
const WORKER_PACK_KEY_ENV: &str = "SIRIUS_REMOTE_PACK_KEY";

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CudaProfile {
    #[default]
    Auto,
    Cuda12,
    Cuda13,
}

impl CudaProfile {
    fn cache_value(self) -> anyhow::Result<&'static str> {
        match self {
            Self::Auto => bail!("the automatic CUDA profile must be resolved before execution"),
            Self::Cuda12 => Ok("cuda12"),
            Self::Cuda13 => Ok("cuda13"),
        }
    }

    fn pixi_environment(self) -> anyhow::Result<&'static str> {
        match self {
            Self::Auto => bail!("the automatic CUDA profile must be resolved before execution"),
            Self::Cuda12 => Ok("cuda12"),
            Self::Cuda13 => Ok("default"),
        }
    }

    fn cuda_architectures(self) -> anyhow::Result<&'static str> {
        match self {
            Self::Auto => bail!("the automatic CUDA profile must be resolved before execution"),
            Self::Cuda12 => Ok(CUDA_12_ARCHITECTURES),
            Self::Cuda13 => Ok(CUDA_13_ARCHITECTURES),
        }
    }

    fn vcpkg_cuda_version(self) -> anyhow::Result<&'static str> {
        match self {
            Self::Auto => bail!("the automatic CUDA profile must be resolved before execution"),
            Self::Cuda12 => Ok("12"),
            Self::Cuda13 => Ok("13"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionTarget {
    pub architecture: String,
    pub target_platform: String,
    pub pixi_environment: String,
    pub cuda_profile: CudaProfile,
    pub cuda_required: bool,
    pub required_cuda_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteInvocation {
    pub schema_version: u32,
    pub benchmark: String,
    pub queries: Vec<String>,
    pub iterations: Option<u32>,
    pub engine: Option<Engine>,
    pub config: Option<PathBuf>,
    pub pin: PinPolicy,
    pub preset: Option<String>,
    pub build_dir: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub verify_data: bool,
    pub remote_repo: PathBuf,
    pub remote_data_root: PathBuf,
    pub cuda_profile: CudaProfile,
    pub ssh_target: Option<String>,
    pub allow_source_difference: bool,
    pub expected_remote_repository: Option<crate::remote::RemoteRepositoryState>,
    pub execution_target: Option<RemoteExecutionTarget>,
}

impl RemoteInvocation {
    pub fn new(
        benchmark: impl Into<String>,
        remote_repo: impl Into<PathBuf>,
        remote_data_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            schema_version: REMOTE_INVOCATION_VERSION,
            benchmark: benchmark.into(),
            queries: Vec::new(),
            iterations: None,
            engine: None,
            config: None,
            pin: PinPolicy::None,
            preset: None,
            build_dir: None,
            data: None,
            verify_data: false,
            remote_repo: remote_repo.into(),
            remote_data_root: remote_data_root.into(),
            cuda_profile: CudaProfile::Auto,
            ssh_target: None,
            allow_source_difference: false,
            expected_remote_repository: None,
            execution_target: None,
        }
    }

    fn validate_client(&self) -> anyhow::Result<()> {
        ensure!(
            self.schema_version == REMOTE_INVOCATION_VERSION,
            "unsupported remote invocation version {}",
            self.schema_version
        );
        ensure!(
            !self.benchmark.trim().is_empty() && !self.benchmark.chars().any(char::is_control),
            "remote benchmark name cannot be empty or contain control characters"
        );
        ensure!(
            self.iterations.is_none_or(|iterations| iterations > 0),
            "remote benchmark iterations must be positive"
        );
        ensure!(
            self.remote_repo.is_absolute(),
            "--remote-repo must be an absolute path"
        );
        ensure!(
            self.remote_data_root.is_absolute(),
            "--remote-data-root must be an absolute path"
        );
        validate_transport_path(&self.remote_repo, "remote repository")?;
        validate_transport_path(&self.remote_data_root, "remote data root")?;
        for (path, label) in [
            (self.config.as_deref(), "remote config"),
            (self.build_dir.as_deref(), "remote build directory"),
            (self.data.as_deref(), "remote dataset"),
        ] {
            if let Some(path) = path {
                ensure!(!path.as_os_str().is_empty(), "{label} path cannot be empty");
                validate_transport_path(path, label)?;
            }
        }
        if let Some(preset) = &self.preset {
            ensure!(
                matches!(preset.as_str(), "release" | "debug" | "relwithdebinfo"),
                "unsupported build preset `{preset}`; expected release, debug, or relwithdebinfo"
            );
        }
        let assets = Assets::resolve(None);
        let benchmark = assets
            .load_bench(&self.benchmark)
            .with_context(|| format!("loading benchmark `{}`", self.benchmark))?;
        let suite = assets.load_suite(&benchmark.bench.suite)?;
        plan::parse_query_selection(&self.queries, &suite.queries)?;
        ensure!(
            self.cuda_profile == CudaProfile::Auto,
            "the client CUDA profile must be `auto`"
        );
        ensure!(
            self.ssh_target.is_none(),
            "the client cannot set remote execution-origin metadata"
        );
        ensure!(
            self.expected_remote_repository.is_none(),
            "the client cannot set expected remote repository metadata"
        );
        ensure!(
            self.execution_target.is_none(),
            "the client cannot set resolved remote execution-target metadata"
        );
        Ok(())
    }

    fn validate_worker(&self) -> anyhow::Result<()> {
        ensure!(
            self.schema_version == REMOTE_INVOCATION_VERSION,
            "unsupported remote invocation version {}",
            self.schema_version
        );
        ensure!(
            matches!(self.cuda_profile, CudaProfile::Cuda12 | CudaProfile::Cuda13),
            "remote invocation did not contain a resolved CUDA profile"
        );
        ensure!(
            self.remote_repo.is_absolute() && self.remote_data_root.is_absolute(),
            "remote repository and data paths must be absolute"
        );
        let target = self
            .ssh_target
            .as_deref()
            .context("remote invocation omitted its SSH execution origin")?;
        SshTarget::new(target)?;
        self.expected_remote_repository
            .as_ref()
            .context("remote invocation omitted its expected repository state")?;
        let execution_target = self
            .execution_target
            .as_ref()
            .context("remote invocation omitted its resolved execution target")?;
        ensure!(
            execution_target.cuda_profile == self.cuda_profile,
            "remote invocation CUDA profile does not match its execution target"
        );
        Ok(())
    }
}

fn validate_transport_path(path: &Path, label: &str) -> anyhow::Result<()> {
    let path = path
        .to_str()
        .with_context(|| format!("{label} path is not valid UTF-8"))?;
    ensure!(
        !path.chars().any(char::is_control),
        "{label} path cannot contain control characters"
    );
    Ok(())
}

#[derive(Debug, Clone)]
pub struct RemoteClientRequest {
    pub target: SshTarget,
    pub local_repo: PathBuf,
    pub local_output: PathBuf,
    pub run_id: RemoteRunId,
    pub invocation: RemoteInvocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDryRun {
    pub target: String,
    pub run_id: String,
    pub compatibility: RemoteCompatibility,
    pub execution_target: RemoteExecutionTarget,
    pub remote_repo: PathBuf,
    pub local_repository: Option<crate::remote::RemoteRepositoryState>,
    pub remote_repository: crate::remote::RemoteRepositoryState,
    pub pack_key: PackCacheKey,
    pub pixi_pack_version: String,
    pub local_pixi_version: Option<String>,
    pub local_pack_cache_hit: Option<bool>,
    pub remote_pack_cache_hit: bool,
    pub planned_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutionOutcome {
    pub target: String,
    pub run_id: String,
    pub remote_repo: PathBuf,
    pub local_repository: Option<crate::remote::RemoteRepositoryState>,
    pub remote_repository: crate::remote::RemoteRepositoryState,
    pub compatibility: RemoteCompatibility,
    pub execution_target: RemoteExecutionTarget,
    pub pack_key: PackCacheKey,
    pub local_pack_cache_hit: Option<bool>,
    pub remote_pack_cache_hit: bool,
    pub status: RemoteRunStatus,
    pub output: Option<PathBuf>,
    pub bundle_status: Option<RunStatus>,
    pub validation_status: Option<ValidationStatus>,
    pub summary: Option<runner::RunSummary>,
    pub failure: Option<RemoteFailure>,
    pub remote_job_retained: bool,
    pub cleanup_warning: Option<String>,
}

impl RemoteExecutionOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == RemoteRunStatus::Completed && self.failure.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteClientOutcome {
    DryRun(RemoteDryRun),
    Executed(RemoteExecutionOutcome),
}

pub struct RemoteExecutionClient<R> {
    executor: RemoteExecutor<R>,
    runner_binary: PathBuf,
}

impl RemoteExecutionClient<SystemCommandRunner> {
    pub fn current() -> anyhow::Result<Self> {
        Self::with_runner(
            SystemCommandRunner,
            PixiPackCache::new(default_pack_cache_root()?),
            env::current_exe().context("locating the current sirius-runner executable")?,
        )
    }
}

impl<R> RemoteExecutionClient<R>
where
    R: CommandRunner,
{
    pub fn with_runner(
        command_runner: R,
        pack_cache: PixiPackCache,
        runner_binary: PathBuf,
    ) -> anyhow::Result<Self> {
        ensure!(
            runner_binary.is_file(),
            "runner binary does not exist: {}",
            runner_binary.display()
        );
        Ok(Self {
            executor: RemoteExecutor::new(command_runner, pack_cache),
            runner_binary,
        })
    }

    pub fn command_runner(&self) -> &R {
        self.executor.command_runner()
    }

    pub fn run(
        &mut self,
        request: &RemoteClientRequest,
        dry_run: bool,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteClientOutcome> {
        if dry_run {
            self.dry_run(request, progress_sink)
                .map(RemoteClientOutcome::DryRun)
        } else {
            self.execute(request, progress_sink)
                .map(RemoteClientOutcome::Executed)
        }
    }

    pub fn dry_run(
        &mut self,
        request: &RemoteClientRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteDryRun> {
        let prepared = self.preflight(request, progress_sink)?;
        let remote_pack_cache_hit =
            self.executor
                .remote_pack_cached(&request.target, &prepared.pack_key, progress_sink)?;
        let local_pack_cache_hit = if remote_pack_cache_hit {
            None
        } else {
            Some(
                self.executor
                    .lookup_local_pack(&prepared.pack, progress_sink)?
                    .is_some(),
            )
        };

        let local_pixi_version = if !remote_pack_cache_hit && local_pack_cache_hit == Some(false) {
            Some(self.executor.probe_local_pixi(progress_sink)?)
        } else {
            None
        };
        let mut planned_actions = Vec::new();
        if remote_pack_cache_hit {
            planned_actions.push("reuse the remote Pixi environment".to_owned());
        } else if local_pack_cache_hit == Some(true) {
            planned_actions.push("upload and unpack the cached local Pixi environment".to_owned());
        } else {
            planned_actions.push(format!(
                "build Pixi environment with pixi-pack {PIXI_PACK_VERSION}, then upload and unpack it"
            ));
        }
        planned_actions.extend([
            "create an isolated remote job directory".to_owned(),
            "upload the current same-architecture runner".to_owned(),
            "handshake, resolve the benchmark plan, and execute it".to_owned(),
            format!(
                "download and atomically publish the result bundle at {}",
                request.local_output.display()
            ),
            "delete the remote job after a verified successful download; retain failures for inspection"
                .to_owned(),
        ]);

        Ok(RemoteDryRun {
            target: request.target.as_str().to_owned(),
            run_id: request.run_id.as_str().to_owned(),
            compatibility: prepared.compatibility,
            execution_target: prepared.execution_target,
            remote_repo: prepared.invocation.remote_repo,
            local_repository: prepared.local_repository,
            remote_repository: prepared.remote_repository,
            pack_key: prepared.pack_key,
            pixi_pack_version: PIXI_PACK_VERSION.to_owned(),
            local_pixi_version,
            local_pack_cache_hit,
            remote_pack_cache_hit,
            planned_actions,
        })
    }

    pub fn execute(
        &mut self,
        request: &RemoteClientRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteExecutionOutcome> {
        let prepared = self.preflight(request, progress_sink)?;
        let transport_archive = transport_archive_path(&request.local_output);
        ensure!(
            !transport_archive.exists(),
            "temporary result path already exists: {}",
            transport_archive.display()
        );
        let archive_cleanup = RemoveFileOnDrop(transport_archive.clone());
        let local_repository = prepared.local_repository.clone();
        let remote_repository = prepared.remote_repository.clone();
        let transport_request = RemoteRunRequest {
            target: request.target.clone(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            pack: prepared.pack,
            runner_binary: self.runner_binary.clone(),
            input_archive: None,
            run_id: request.run_id.clone(),
            plan_version: REMOTE_INVOCATION_VERSION,
            payload: prepared.invocation,
            result_archive: transport_archive,
        };
        let outcome = self
            .executor
            .execute_preprobed(
                &transport_request,
                prepared.compatibility.clone(),
                progress_sink,
            )
            .with_context(|| {
                format!(
                    "remote run {} on {} failed; if its job was created, inspect {}",
                    request.run_id.as_str(),
                    request.target.as_str(),
                    remote_job_location(request)
                )
            })?;
        let output = if let Some(archive) = outcome.local_result_archive.as_deref() {
            extract_result_archive(archive, &request.local_output).with_context(|| {
                format!(
                    "extracting the downloaded result failed; remote job retained at {}",
                    remote_job_location(request)
                )
            })?;
            Some(request.local_output.clone())
        } else {
            None
        };
        drop(archive_cleanup);

        let bundle = output
            .as_deref()
            .map(read_run_record)
            .transpose()
            .with_context(|| {
                format!(
                    "reading the downloaded result bundle failed; remote job retained at {}",
                    remote_job_location(request)
                )
            })?;
        let completed = outcome.envelope.status == RemoteRunStatus::Completed
            && outcome.envelope.error.is_none()
            && output.is_some()
            && bundle.is_some();
        let (remote_job_retained, cleanup_warning) = if completed {
            match self
                .executor
                .cleanup_job(&request.target, &request.run_id, progress_sink)
            {
                Ok(()) => (false, None),
                Err(error) => {
                    let warning = format!("{error:#}");
                    progress_sink.emit(crate::remote::ProgressEvent {
                        stage: RemoteStage::CleanupJob,
                        message: format!(
                            "Remote result is safe locally, but cleanup failed; retaining run {}",
                            request.run_id.as_str()
                        ),
                    });
                    (true, Some(warning))
                }
            }
        } else {
            (true, None)
        };
        Ok(outcome_for_user(
            outcome,
            UserOutcomeInputs {
                execution_target: prepared.execution_target,
                request,
                local_repository,
                remote_repository,
                output,
                record: bundle.as_ref(),
                remote_job_retained,
                cleanup_warning,
            },
        ))
    }

    fn preflight(
        &mut self,
        request: &RemoteClientRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<PreparedExecution> {
        request.invocation.validate_client()?;
        ensure!(
            !request.local_output.exists(),
            "result bundle already exists: {}",
            request.local_output.display()
        );
        ensure!(
            request.local_repo.join("pixi.toml").is_file(),
            "local repository has no pixi.toml: {}",
            request.local_repo.display()
        );
        ensure!(
            request.local_repo.join("pixi.lock").is_file(),
            "local repository has no pixi.lock: {}",
            request.local_repo.display()
        );

        let requires_cuda = invocation_requires_cuda(&request.invocation)?;
        let compatibility = self.executor.probe(&request.target, progress_sink)?;
        let execution_target = select_execution_target_for_workload(&compatibility, requires_cuda)?;
        let remote_repository = self.executor.probe_repository(
            &request.target,
            &request.invocation.remote_repo,
            progress_sink,
        )?;
        if request.invocation.config.is_some()
            || request.invocation.build_dir.is_some()
            || request.invocation.data.is_some()
        {
            self.executor.probe_run_paths(
                &request.target,
                &request.invocation.remote_repo,
                request.invocation.config.as_deref(),
                request.invocation.build_dir.as_deref(),
                request.invocation.data.as_deref(),
                progress_sink,
            )?;
        }

        let mut invocation = request.invocation.clone();
        invocation.cuda_profile = execution_target.cuda_profile;
        invocation.execution_target = Some(execution_target.clone());
        invocation.ssh_target = Some(request.target.as_str().to_owned());
        let pack = PixiPackRequest {
            manifest_path: request.local_repo.join("pixi.toml"),
            lock_path: request.local_repo.join("pixi.lock"),
            environment: execution_target.pixi_environment.clone(),
            target_platform: execution_target.target_platform.clone(),
            cuda_profile: execution_target.cuda_profile.cache_value()?.to_owned(),
            cuda_required: execution_target.cuda_required,
            pixi_pack_version: PIXI_PACK_VERSION.to_owned(),
            format: PixiPackFormat::SelfExtractingShellV1,
        };
        let pack_inputs = pack.key_inputs()?;
        ensure!(
            pack_inputs.manifest_digest == remote_repository.manifest_sha256
                && pack_inputs.lock_digest == remote_repository.lock_sha256,
            "local and remote pixi.toml/pixi.lock differ; synchronize the remote checkout before running"
        );
        let local_repository = if request.invocation.allow_source_difference {
            None
        } else {
            let local = local_repository_state(
                &request.local_repo,
                pack_inputs.manifest_digest,
                pack_inputs.lock_digest,
            )?;
            ensure!(
                !local.git_dirty,
                "local checkout has uncommitted or untracked changes; commit/stash them or pass --allow-remote-source-difference"
            );
            ensure!(
                !remote_repository.git_dirty,
                "remote checkout has uncommitted or untracked changes; commit/stash them or pass --allow-remote-source-difference"
            );
            ensure!(
                local.git_commit == remote_repository.git_commit,
                "local and remote Git commits differ (local {}, remote {}); synchronize them or pass --allow-remote-source-difference",
                local.git_commit,
                remote_repository.git_commit
            );
            Some(local)
        };
        invocation.expected_remote_repository = Some(remote_repository.clone());
        let pack_key = pack_inputs.cache_key();
        Ok(PreparedExecution {
            compatibility,
            execution_target,
            invocation,
            pack,
            pack_key,
            local_repository,
            remote_repository,
        })
    }
}

struct PreparedExecution {
    compatibility: RemoteCompatibility,
    execution_target: RemoteExecutionTarget,
    invocation: RemoteInvocation,
    pack: PixiPackRequest,
    pack_key: PackCacheKey,
    local_repository: Option<crate::remote::RemoteRepositoryState>,
    remote_repository: crate::remote::RemoteRepositoryState,
}

fn local_repository_state(
    repository: &Path,
    manifest_sha256: crate::remote::Sha256Digest,
    lock_sha256: crate::remote::Sha256Digest,
) -> anyhow::Result<crate::remote::RemoteRepositoryState> {
    let git_commit = crate::repository::git_output(repository, &["rev-parse", "HEAD"])?;
    ensure!(
        matches!(git_commit.len(), 40 | 64)
            && git_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "local checkout returned an invalid Git commit"
    );
    Ok(crate::remote::RemoteRepositoryState {
        manifest_sha256,
        lock_sha256,
        git_commit,
        git_dirty: crate::repository::is_dirty(repository)?,
    })
}

fn remote_job_location(request: &RemoteClientRequest) -> String {
    format!(
        "{}:~/.cache/sirius-runner/v1/jobs/{}",
        request.target.as_str(),
        request.run_id.as_str()
    )
}

struct UserOutcomeInputs<'a> {
    execution_target: RemoteExecutionTarget,
    request: &'a RemoteClientRequest,
    local_repository: Option<crate::remote::RemoteRepositoryState>,
    remote_repository: crate::remote::RemoteRepositoryState,
    output: Option<PathBuf>,
    record: Option<&'a RunRecord>,
    remote_job_retained: bool,
    cleanup_warning: Option<String>,
}

fn outcome_for_user(
    outcome: RemoteRunOutcome,
    inputs: UserOutcomeInputs<'_>,
) -> RemoteExecutionOutcome {
    let summary = inputs
        .record
        .filter(|record| {
            outcome.envelope.status == RemoteRunStatus::Completed
                && record.status == RunStatus::Complete
        })
        .zip(inputs.output.as_deref())
        .map(|(record, output)| runner::summarize(record, output));
    RemoteExecutionOutcome {
        target: inputs.request.target.as_str().to_owned(),
        run_id: inputs.request.run_id.as_str().to_owned(),
        remote_repo: inputs.request.invocation.remote_repo.clone(),
        local_repository: inputs.local_repository,
        remote_repository: inputs.remote_repository,
        compatibility: outcome.compatibility,
        execution_target: inputs.execution_target,
        pack_key: outcome.pack_key,
        local_pack_cache_hit: outcome.local_pack.map(|pack| pack.cache_hit),
        remote_pack_cache_hit: outcome.remote_pack_cache_hit,
        status: outcome.envelope.status,
        output: inputs.output,
        bundle_status: inputs.record.map(|record| record.status),
        validation_status: inputs.record.map(|record| record.validation_status),
        summary,
        failure: outcome.envelope.error,
        remote_job_retained: inputs.remote_job_retained,
        cleanup_warning: inputs.cleanup_warning,
    }
}

pub fn default_pack_cache_root() -> anyhow::Result<PathBuf> {
    let root = if let Some(root) = env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(root)
    } else {
        PathBuf::from(
            env::var_os("HOME")
                .context("neither XDG_CACHE_HOME nor HOME is set; cannot locate the pack cache")?,
        )
        .join(".cache")
    };
    Ok(root.join("sirius-runner/v1/pixi-packs"))
}

pub fn select_execution_target(
    compatibility: &RemoteCompatibility,
) -> anyhow::Result<RemoteExecutionTarget> {
    select_execution_target_for_workload(compatibility, true)
}

pub(crate) fn select_execution_target_for_workload(
    compatibility: &RemoteCompatibility,
    requires_cuda: bool,
) -> anyhow::Result<RemoteExecutionTarget> {
    ensure!(
        cfg!(target_os = "linux"),
        "remote execution v0 can upload the current runner only from Linux"
    );
    let local_glibc = local_glibc_version()?;
    select_execution_target_for_host(
        compatibility,
        env::consts::ARCH,
        &local_glibc,
        requires_cuda,
    )
}

fn select_execution_target_for_host(
    compatibility: &RemoteCompatibility,
    local_architecture: &str,
    local_glibc: &str,
    requires_cuda: bool,
) -> anyhow::Result<RemoteExecutionTarget> {
    ensure!(
        compatibility.os.eq_ignore_ascii_case("linux"),
        "remote execution requires Linux; remote reported `{}`",
        compatibility.os
    );
    ensure!(
        compatibility.flock_available,
        "remote host is missing `flock`, which is required to serialize benchmark work"
    );
    let local_architecture = normalize_architecture(local_architecture)?;
    let remote_architecture = normalize_architecture(&compatibility.architecture)?;
    ensure!(
        local_architecture == remote_architecture,
        "remote execution v0 uploads the current runner and cannot cross architectures: local is `{local_architecture}`, remote is `{remote_architecture}`"
    );
    ensure_glibc_compatible(
        local_glibc,
        compatibility
            .glibc_version
            .as_deref()
            .context("remote did not report a glibc version")?,
    )?;
    let architecture = match remote_architecture {
        "x86_64" => "linux-64",
        "aarch64" => "linux-aarch64",
        _ => unreachable!("architecture was normalized"),
    };
    if !requires_cuda {
        let cuda_profile = CudaProfile::Cuda12;
        return Ok(RemoteExecutionTarget {
            architecture: remote_architecture.to_owned(),
            target_platform: architecture.to_owned(),
            pixi_environment: cuda_profile.pixi_environment()?.to_owned(),
            cuda_profile,
            cuda_required: false,
            required_cuda_version: None,
        });
    }
    ensure!(
        compatibility.nvidia_driver_version.is_some(),
        "remote did not report an NVIDIA driver required for a Sirius benchmark"
    );
    let cuda_version = compatibility
        .nvidia_cuda_version
        .as_deref()
        .context("remote did not report its CUDA driver compatibility")?;
    let reported_cuda = parse_numeric_version(cuda_version)
        .with_context(|| format!("parsing remote CUDA compatibility `{cuda_version}`"))?;
    let (cuda_profile, required_cuda) = if reported_cuda >= CUDA_13_TOOLKIT {
        (CudaProfile::Cuda13, CUDA_13_TOOLKIT)
    } else if reported_cuda >= CUDA_12_TOOLKIT {
        (CudaProfile::Cuda12, CUDA_12_TOOLKIT)
    } else {
        bail!(
            "remote CUDA compatibility {cuda_version} is too old; the locked CUDA 12 environment requires {}.{} or newer",
            CUDA_12_TOOLKIT.0,
            CUDA_12_TOOLKIT.1
        )
    };
    let target_platform = architecture.to_owned();
    compatibility.ensure_compatible(&target_platform, cuda_profile.cache_value()?)?;
    Ok(RemoteExecutionTarget {
        architecture: remote_architecture.to_owned(),
        target_platform,
        pixi_environment: cuda_profile.pixi_environment()?.to_owned(),
        cuda_profile,
        cuda_required: true,
        required_cuda_version: Some(format!("{}.{}", required_cuda.0, required_cuda.1)),
    })
}

fn invocation_requires_cuda(invocation: &RemoteInvocation) -> anyhow::Result<bool> {
    let engine = match invocation.engine {
        Some(engine) => engine,
        None => {
            Assets::resolve(None)
                .load_bench(&invocation.benchmark)
                .with_context(|| format!("loading benchmark `{}`", invocation.benchmark))?
                .engine
                .engine
        }
    };
    if engine == Engine::Duckdb {
        ensure!(
            invocation.pin == PinPolicy::None,
            "--pin applies only to Sirius runs"
        );
        ensure!(
            invocation.config.is_none(),
            "--config applies only to Sirius runs"
        );
        ensure!(
            invocation.preset.is_none() && invocation.build_dir.is_none(),
            "--preset and --build-dir apply only to Sirius runs"
        );
        Ok(false)
    } else {
        Ok(true)
    }
}

fn normalize_architecture(architecture: &str) -> anyhow::Result<&'static str> {
    match architecture.trim().to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        _ => bail!("unsupported runner architecture `{architecture}`"),
    }
}

fn local_glibc_version() -> anyhow::Result<String> {
    ensure!(
        cfg!(target_env = "gnu"),
        "remote execution v0 requires a GNU/Linux runner binary; static and musl runner portability is not yet verified"
    );
    let output = Command::new("getconf")
        .arg("GNU_LIBC_VERSION")
        .output()
        .context("running `getconf GNU_LIBC_VERSION` for the local runner")?;
    ensure!(
        output.status.success(),
        "could not determine the glibc required by the local runner"
    );
    let version = std::str::from_utf8(&output.stdout)
        .context("local getconf returned non-UTF-8 output")?
        .trim();
    parse_numeric_version(version)
        .with_context(|| format!("parsing local glibc version `{version}`"))?;
    Ok(version.to_owned())
}

fn ensure_glibc_compatible(local: &str, remote: &str) -> anyhow::Result<()> {
    let local_version =
        parse_numeric_version(local).with_context(|| format!("parsing local glibc `{local}`"))?;
    let remote_version = parse_numeric_version(remote)
        .with_context(|| format!("parsing remote glibc `{remote}`"))?;
    ensure!(
        remote_version >= local_version,
        "the uploaded runner requires glibc {}.{}, but the remote has {}.{}; build the runner on an older-compatible system or use a static runner",
        local_version.0,
        local_version.1,
        remote_version.0,
        remote_version.1
    );
    Ok(())
}

fn parse_numeric_version(value: &str) -> anyhow::Result<(u32, u32)> {
    let token = value
        .split_whitespace()
        .find(|token| token.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .context("version contains no numeric component")?
        .trim_matches(|character: char| !character.is_ascii_digit() && character != '.');
    let mut components = token.split('.');
    let major = components
        .next()
        .context("version has no major component")?
        .parse()
        .context("version major component is invalid")?;
    let minor = components
        .next()
        .unwrap_or("0")
        .parse()
        .context("version minor component is invalid")?;
    Ok((major, minor))
}

fn transport_archive_path(output: &Path) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(
        ".sirius-runner-result-{}-{}.tar",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

struct RemoveFileOnDrop(PathBuf);

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct RemoveDirectoryOnDrop {
    path: PathBuf,
    armed: bool,
}

impl Drop for RemoveDirectoryOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub fn extract_result_archive(archive: &Path, destination: &Path) -> anyhow::Result<()> {
    ensure!(
        archive.is_file(),
        "result archive does not exist: {}",
        archive.display()
    );
    ensure!(
        !destination.exists(),
        "result bundle already exists: {}",
        destination.display()
    );
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating result parent {}", parent.display()))?;
    let temporary = parent.join(format!(
        ".sirius-runner-bundle-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&temporary)
        .with_context(|| format!("creating temporary bundle {}", temporary.display()))?;
    let mut cleanup = RemoveDirectoryOnDrop {
        path: temporary.clone(),
        armed: true,
    };

    let input = File::open(archive)
        .with_context(|| format!("opening result archive {}", archive.display()))?;
    let mut archive = tar::Archive::new(input);
    let mut paths = HashSet::new();
    for entry in archive.entries().context("reading result archive")? {
        let mut entry = entry.context("reading result archive entry")?;
        let path = entry
            .path()
            .context("reading result archive path")?
            .into_owned();
        validate_archive_path(&path)?;
        ensure!(
            paths.insert(path.clone()),
            "result archive contains duplicate path {}",
            path.display()
        );
        let entry_type = entry.header().entry_type();
        ensure!(
            entry_type.is_file() || entry_type.is_dir(),
            "result archive contains unsupported entry type at {}",
            path.display()
        );
        ensure!(
            entry
                .unpack_in(&temporary)
                .with_context(|| format!("extracting {}", path.display()))?,
            "result archive entry escapes the destination: {}",
            path.display()
        );
    }
    ensure!(
        temporary.join("run.json").is_file(),
        "result archive does not contain run.json at its root"
    );
    ensure!(
        !destination.exists(),
        "result bundle appeared while the archive was being extracted: {}",
        destination.display()
    );
    fs::rename(&temporary, destination).with_context(|| {
        format!(
            "publishing result bundle {} to {}",
            temporary.display(),
            destination.display()
        )
    })?;
    cleanup.armed = false;
    Ok(())
}

fn validate_archive_path(path: &Path) -> anyhow::Result<()> {
    ensure!(!path.as_os_str().is_empty(), "result archive path is empty");
    let mut normal_components = 0;
    for component in path.components() {
        match component {
            Component::Normal(_) => normal_components += 1,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("result archive contains unsafe path {}", path.display())
            }
        }
    }
    ensure!(
        normal_components > 0,
        "result archive contains an empty relative path"
    );
    Ok(())
}

fn read_run_record(bundle: &Path) -> anyhow::Result<RunRecord> {
    serde_json::from_slice(
        &fs::read(bundle.join("run.json"))
            .with_context(|| format!("reading {}", bundle.join("run.json").display()))?,
    )
    .with_context(|| format!("parsing {}", bundle.join("run.json").display()))
}

pub fn run_hidden_worker(action: &str) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut progress = Progress::stderr(1);
    serve_hidden_worker(action, &mut stdin.lock(), &mut stdout.lock(), &mut progress)
}

pub fn serve_hidden_worker(
    action: &str,
    input: &mut impl Read,
    output: &mut impl Write,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    match action {
        "handshake" => {
            let request: HandshakeRequest =
                serde_json::from_reader(input).context("parsing remote handshake request")?;
            let response = worker_handshake(&request)?;
            serde_json::to_writer(&mut *output, &response)?;
            writeln!(output)?;
        }
        "run" => {
            let plan: RemotePlan<RemoteInvocation> =
                serde_json::from_reader(input).context("parsing remote invocation")?;
            let envelope = worker_run(plan, reporter);
            serde_json::to_writer(&mut *output, &envelope)?;
            writeln!(output)?;
        }
        _ => bail!("unsupported hidden remote worker action `{action}`"),
    }
    output.flush()?;
    Ok(())
}

fn worker_handshake(request: &HandshakeRequest) -> anyhow::Result<HandshakeResponse> {
    ensure!(
        request.protocol_version == REMOTE_PROTOCOL_VERSION,
        "unsupported remote protocol version {}",
        request.protocol_version
    );
    ensure!(
        request.plan_version == REMOTE_INVOCATION_VERSION,
        "unsupported remote invocation version {}",
        request.plan_version
    );
    ensure!(
        request.client_version == env!("CARGO_PKG_VERSION"),
        "client version `{}` does not match runner version `{}`",
        request.client_version,
        env!("CARGO_PKG_VERSION")
    );
    ensure_worker_pack_key(&request.pack_key)?;
    let runner = ArtifactDescriptor::from_file(
        &env::current_exe().context("locating the remote runner executable")?,
    )?;
    ensure!(
        runner == request.runner,
        "uploaded runner checksum does not match the handshake"
    );
    Ok(HandshakeResponse {
        protocol_version: REMOTE_PROTOCOL_VERSION,
        runner_version: env!("CARGO_PKG_VERSION").to_owned(),
        plan_version: REMOTE_INVOCATION_VERSION,
        pack_key: request.pack_key.clone(),
        runner,
    })
}

fn worker_run(
    remote_plan: RemotePlan<RemoteInvocation>,
    reporter: &mut impl Reporter,
) -> ResultEnvelope<ArtifactDescriptor> {
    let run_id = remote_plan.run_id.clone();
    match worker_run_inner(&remote_plan, reporter) {
        Ok((result, None)) => ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: REMOTE_RESULT_VERSION,
            run_id,
            status: RemoteRunStatus::Completed,
            result: Some(result),
            error: None,
        },
        Ok((result, Some(error))) => ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: REMOTE_RESULT_VERSION,
            run_id,
            status: RemoteRunStatus::Failed,
            result: Some(result),
            error: Some(RemoteFailure {
                code: "remote_run_failed".to_owned(),
                message: format!("{error:#}"),
            }),
        },
        Err(error) => ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: REMOTE_RESULT_VERSION,
            run_id,
            status: RemoteRunStatus::Failed,
            result: None,
            error: Some(RemoteFailure {
                code: "remote_run_failed".to_owned(),
                message: format!("{error:#}"),
            }),
        },
    }
}

fn worker_run_inner(
    remote_plan: &RemotePlan<RemoteInvocation>,
    reporter: &mut impl Reporter,
) -> anyhow::Result<(ArtifactDescriptor, Option<anyhow::Error>)> {
    ensure!(
        remote_plan.protocol_version == REMOTE_PROTOCOL_VERSION,
        "unsupported remote protocol version {}",
        remote_plan.protocol_version
    );
    ensure!(
        remote_plan.plan_version == REMOTE_INVOCATION_VERSION,
        "unsupported remote invocation version {}",
        remote_plan.plan_version
    );
    ensure!(
        remote_plan.input.is_none(),
        "remote benchmark invocation does not accept an input archive"
    );
    ensure_worker_pack_key(&remote_plan.pack_key)?;
    remote_plan.payload.validate_worker()?;
    ensure_remote_repository_unchanged(
        &remote_plan.payload,
        "before resolving the benchmark plan",
        reporter,
    )?;
    let job = worker_job_directory(&remote_plan.run_id)?;
    let bundle = job.join("bundle");
    ensure!(
        !bundle.exists(),
        "remote result bundle already exists: {}",
        bundle.display()
    );

    let profile = remote_plan.payload.cuda_profile;
    let execution_target = remote_plan
        .payload
        .execution_target
        .as_ref()
        .context("remote invocation omitted its resolved execution target")?;
    // The hidden worker is a short-lived, single-threaded process, so changing
    // its process environment cannot race another thread.
    unsafe {
        env::set_var("SIRIUS_RUNNER_PACKED", "1");
        env::set_var("CUDAARCHS", profile.cuda_architectures()?);
        env::set_var("VCPKG_CUDA_VERSION", profile.vcpkg_cuda_version()?);
        env::set_var("PIXI_PROJECT_ROOT", &remote_plan.payload.remote_repo);
        let conda_prefix =
            env::var("CONDA_PREFIX").context("the packed environment did not set CONDA_PREFIX")?;
        env::set_var(
            "SCCACHE_BASEDIRS",
            format!(
                "{}:{conda_prefix}",
                remote_plan.payload.remote_repo.display()
            ),
        );
    }
    let overrides = RunOverrides {
        name: remote_plan.payload.benchmark.clone(),
        repo_root: Some(remote_plan.payload.remote_repo.clone()),
        data_root: Some(remote_plan.payload.remote_data_root.clone()),
        queries: remote_plan.payload.queries.clone(),
        iterations: remote_plan.payload.iterations,
        engine: remote_plan.payload.engine,
        config: remote_plan.payload.config.clone(),
        pin: remote_plan.payload.pin,
        preset: remote_plan.payload.preset.clone(),
        build_dir: remote_plan.payload.build_dir.clone(),
        data: remote_plan.payload.data.clone(),
        verify_data: remote_plan.payload.verify_data,
        output: Some(bundle.clone()),
    };
    let mut plan = plan::resolve(&Assets::resolve(None), overrides)
        .context("resolving the benchmark on the remote checkout")?;
    ensure_remote_repository_unchanged(
        &remote_plan.payload,
        "after resolving the benchmark plan",
        reporter,
    )?;
    let expected_repository = remote_plan
        .payload
        .expected_remote_repository
        .as_ref()
        .context("remote invocation omitted its expected repository state")?;
    plan.origin = ExecutionOrigin::Ssh {
        target: remote_plan
            .payload
            .ssh_target
            .clone()
            .context("remote invocation omitted its SSH target")?,
        run_id: remote_plan.run_id.as_str().to_owned(),
        remote_repo: remote_plan.payload.remote_repo.clone(),
    };
    plan.sources.insert(
        "remote_source_validation".to_owned(),
        if remote_plan.payload.allow_source_difference {
            "explicit source-difference override".to_owned()
        } else {
            "clean matching local and remote Git commits".to_owned()
        },
    );
    plan.sources.insert(
        "remote_git_commit".to_owned(),
        expected_repository.git_commit.clone(),
    );
    plan.sources.insert(
        "remote_git_status".to_owned(),
        if expected_repository.git_dirty {
            "dirty"
        } else {
            "clean"
        }
        .to_owned(),
    );
    plan.sources.insert(
        "remote_manifest_sha256".to_owned(),
        expected_repository.manifest_sha256.to_string(),
    );
    plan.sources.insert(
        "remote_lock_sha256".to_owned(),
        expected_repository.lock_sha256.to_string(),
    );
    plan.sources.insert(
        "remote_pixi_pack_key".to_owned(),
        remote_plan.pack_key.to_string(),
    );
    plan.sources.insert(
        "remote_target_platform".to_owned(),
        execution_target.target_platform.clone(),
    );
    plan.sources.insert(
        "remote_pixi_environment".to_owned(),
        execution_target.pixi_environment.clone(),
    );
    plan.sources.insert(
        "remote_cuda_profile".to_owned(),
        if execution_target.cuda_required {
            profile.cache_value()?
        } else {
            "cpu"
        }
        .to_owned(),
    );
    let execution = runner::execute_with_final_check(plan, reporter, |reporter| {
        ensure_remote_repository_unchanged(
            &remote_plan.payload,
            "before finalizing the benchmark bundle",
            reporter,
        )
    });
    ensure!(
        bundle.join("run.json").is_file(),
        "remote runner did not leave a result bundle"
    );
    let result = create_result_archive(&bundle, &job.join("result.tar"))?;
    Ok((result, execution.err()))
}

fn ensure_worker_pack_key(expected: &PackCacheKey) -> anyhow::Result<()> {
    let active = env::var(WORKER_PACK_KEY_ENV)
        .with_context(|| format!("{WORKER_PACK_KEY_ENV} is not set"))?;
    ensure!(
        active == expected.as_str(),
        "active Pixi pack does not match the remote request"
    );
    Ok(())
}

fn ensure_remote_repository_unchanged(
    invocation: &RemoteInvocation,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    reporter.status(&format!("Checking remote source state {phase}"))?;
    let expected = invocation
        .expected_remote_repository
        .as_ref()
        .context("remote invocation omitted its expected repository state")?;
    let current = repository_state(&invocation.remote_repo)?;
    ensure!(
        current == *expected,
        "remote checkout {} changed after client preflight; refusing to use potentially mixed sources",
        invocation.remote_repo.display()
    );
    Ok(())
}

fn repository_state(repository: &Path) -> anyhow::Result<crate::remote::RemoteRepositoryState> {
    let manifest = repository.join("pixi.toml");
    let lock = repository.join("pixi.lock");
    ensure!(
        manifest.is_file() && lock.is_file(),
        "remote checkout no longer contains pixi.toml and pixi.lock"
    );
    local_repository_state(
        repository,
        crate::remote::Sha256Digest::from_file(&manifest)?,
        crate::remote::Sha256Digest::from_file(&lock)?,
    )
}

fn worker_job_directory(run_id: &RemoteRunId) -> anyhow::Result<PathBuf> {
    let job = PathBuf::from(
        env::var_os(WORKER_JOB_ENV).with_context(|| format!("{WORKER_JOB_ENV} is not set"))?,
    );
    let job = fs::canonicalize(&job)
        .with_context(|| format!("resolving remote job directory {}", job.display()))?;
    ensure!(
        job.file_name() == Some(OsStr::new(run_id.as_str())),
        "remote job directory does not match run ID"
    );
    Ok(job)
}

fn create_result_archive(bundle: &Path, destination: &Path) -> anyhow::Result<ArtifactDescriptor> {
    ensure!(
        !destination.exists(),
        "remote result archive already exists: {}",
        destination.display()
    );
    let temporary = destination.with_extension(format!(
        "partial-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let cleanup = RemoveFileOnDrop(temporary.clone());
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("creating result archive {}", temporary.display()))?;
    let mut builder = tar::Builder::new(file);
    builder.follow_symlinks(false);
    append_archive_directory(&mut builder, bundle, Path::new(""))?;
    let file = builder.into_inner().context("finishing result archive")?;
    file.sync_all().context("syncing result archive")?;
    fs::rename(&temporary, destination).with_context(|| {
        format!(
            "publishing result archive {} to {}",
            temporary.display(),
            destination.display()
        )
    })?;
    drop(cleanup);
    ArtifactDescriptor::from_file(destination)
}

fn append_archive_directory<W: Write>(
    builder: &mut tar::Builder<W>,
    source: &Path,
    relative: &Path,
) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(source)
        .with_context(|| format!("reading result bundle {}", source.display()))?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative_path = relative.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("reading result entry {}", path.display()))?;
        if metadata.is_dir() {
            builder
                .append_dir(&relative_path, &path)
                .with_context(|| format!("archiving {}", path.display()))?;
            append_archive_directory(builder, &path, &relative_path)?;
        } else if metadata.is_file() {
            builder
                .append_path_with_name(&path, &relative_path)
                .with_context(|| format!("archiving {}", path.display()))?;
        } else {
            bail!(
                "result bundle contains an unsupported filesystem entry: {}",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use tempfile::TempDir;

    use super::*;
    use crate::remote::{CommandOutputTarget, CommandSpec, NoopProgress, ProcessOutput};

    #[test]
    fn selects_profile_platform_and_rejects_cross_architecture() {
        let remote = compatibility("x86_64", "13.2", "glibc 2.39");
        let selected =
            select_execution_target_for_host(&remote, "amd64", "glibc 2.38", true).unwrap();
        assert_eq!(selected.cuda_profile, CudaProfile::Cuda13);
        assert_eq!(selected.pixi_environment, "default");
        assert_eq!(selected.target_platform, "linux-64");

        let cuda12 = select_execution_target_for_host(
            &compatibility("aarch64", "12.9", "glibc 2.39"),
            "arm64",
            "glibc 2.38",
            true,
        )
        .unwrap();
        assert_eq!(cuda12.cuda_profile, CudaProfile::Cuda12);
        assert_eq!(cuda12.pixi_environment, "cuda12");
        assert_eq!(cuda12.target_platform, "linux-aarch64");

        let fallback = select_execution_target_for_host(
            &compatibility("x86_64", "13.1", "glibc 2.39"),
            "x86_64",
            "glibc 2.38",
            true,
        )
        .unwrap();
        assert_eq!(fallback.cuda_profile, CudaProfile::Cuda12);
        assert!(
            select_execution_target_for_host(
                &compatibility("x86_64", "12.8", "glibc 2.39"),
                "x86_64",
                "glibc 2.38",
                true,
            )
            .is_err()
        );

        let cpu_only = RemoteCompatibility {
            os: "Linux".to_owned(),
            architecture: "x86_64".to_owned(),
            glibc_version: Some("glibc 2.39".to_owned()),
            nvidia_driver_version: None,
            nvidia_cuda_version: None,
            nvidia_compute_capabilities: Vec::new(),
            flock_available: true,
        };
        let cpu_target =
            select_execution_target_for_host(&cpu_only, "x86_64", "glibc 2.38", false).unwrap();
        assert!(!cpu_target.cuda_required);
        assert_eq!(cpu_target.target_platform, "linux-64");

        let error =
            select_execution_target_for_host(&remote, "aarch64", "glibc 2.38", true).unwrap_err();
        assert!(error.to_string().contains("cannot cross architectures"));
    }

    #[test]
    fn rejects_runner_built_against_newer_glibc() {
        let error = ensure_glibc_compatible("glibc 2.39", "glibc 2.35").unwrap_err();
        assert!(error.to_string().contains("requires glibc 2.39"));
        ensure_glibc_compatible("glibc 2.35", "glibc 2.39").unwrap();
    }

    #[test]
    fn explicit_duckdb_runs_do_not_require_a_remote_gpu() {
        let mut invocation = RemoteInvocation::new("tpch-sf1", "/srv/sirius", "/datasets");
        invocation.engine = Some(Engine::Duckdb);
        assert!(!invocation_requires_cuda(&invocation).unwrap());

        invocation.config = Some(PathBuf::from("config.yaml"));
        assert!(invocation_requires_cuda(&invocation).is_err());
    }

    #[test]
    fn packed_cuda_activation_matches_the_workspace_manifest() {
        let manifest: toml::Value = toml::from_str(include_str!("../../../../pixi.toml")).unwrap();
        for (feature, profile, architectures, vcpkg_version) in [
            ("cuda12", CudaProfile::Cuda12, CUDA_12_ARCHITECTURES, "12"),
            ("cuda13", CudaProfile::Cuda13, CUDA_13_ARCHITECTURES, "13"),
        ] {
            let activation = &manifest["feature"][feature]["activation"]["env"];
            assert_eq!(activation["CUDAARCHS"].as_str().unwrap(), architectures);
            assert_eq!(
                activation["VCPKG_CUDA_VERSION"].as_str().unwrap(),
                vcpkg_version
            );
            assert_eq!(profile.cuda_architectures().unwrap(), architectures);
            assert_eq!(profile.vcpkg_cuda_version().unwrap(), vcpkg_version);
        }
    }

    #[test]
    fn client_rejects_invalid_query_and_preset_before_remote_work() {
        let mut invocation = RemoteInvocation::new("tpch-sf1", "/srv/sirius", "/datasets");
        invocation.queries = vec!["q99".to_owned()];
        assert!(invocation.validate_client().is_err());

        invocation.queries = vec!["q1".to_owned()];
        invocation.preset = Some("surprise".to_owned());
        assert!(invocation.validate_client().is_err());
    }

    #[test]
    fn result_archive_is_published_at_the_exact_destination() {
        let root = TempDir::new().unwrap();
        let bundle = root.path().join("bundle");
        fs::create_dir(&bundle).unwrap();
        fs::create_dir(bundle.join("logs")).unwrap();
        fs::write(bundle.join("run.json"), b"{}").unwrap();
        fs::write(bundle.join("runtimes.csv"), b"header\n").unwrap();
        let archive = root.path().join("result.tar");
        create_result_archive(&bundle, &archive).unwrap();
        let destination = root.path().join("exact-output");

        extract_result_archive(&archive, &destination).unwrap();

        assert_eq!(fs::read(destination.join("run.json")).unwrap(), b"{}");
        assert!(destination.join("logs").is_dir());
        assert!(!destination.join("bundle").exists());
    }

    #[test]
    fn extraction_rejects_traversal_and_existing_destination() {
        let root = TempDir::new().unwrap();
        let archive = root.path().join("malicious.tar");
        write_raw_tar(&archive, "../outside", b"bad");
        let destination = root.path().join("output");

        assert!(extract_result_archive(&archive, &destination).is_err());
        assert!(!root.path().join("outside").exists());

        fs::create_dir(&destination).unwrap();
        let error = extract_result_archive(&archive, &destination).unwrap_err();
        assert!(error.to_string().contains("already exists"));
    }

    #[test]
    fn worker_handshake_checks_active_pack_and_versions() {
        let executable = env::current_exe().unwrap();
        let descriptor = ArtifactDescriptor::from_file(&executable).unwrap();
        let inputs = crate::remote::PackKeyInputs {
            manifest_digest: crate::remote::Sha256Digest::from_bytes(b"manifest"),
            lock_digest: crate::remote::Sha256Digest::from_bytes(b"lock"),
            environment: "default".to_owned(),
            target_platform: "linux-64".to_owned(),
            cuda_profile: "cuda13".to_owned(),
            pixi_pack_version: PIXI_PACK_VERSION.to_owned(),
            format: PixiPackFormat::SelfExtractingShellV1,
        };
        let key = inputs.cache_key();
        // This test mutates process environment and therefore keeps its scope
        // to a single synchronous handshake.
        unsafe {
            env::set_var(WORKER_PACK_KEY_ENV, key.as_str());
        }
        let request = HandshakeRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
            plan_version: REMOTE_INVOCATION_VERSION,
            pack_key: key,
            runner: descriptor,
        };

        worker_handshake(&request).unwrap();
        let mut invalid = request;
        invalid.plan_version += 1;
        assert!(worker_handshake(&invalid).is_err());
        unsafe {
            env::remove_var(WORKER_PACK_KEY_ENV);
        }
    }

    #[test]
    fn dry_run_only_uses_read_only_remote_probes_and_does_not_create_cache() {
        let root = TempDir::new().unwrap();
        let local_repo = root.path().join("repo");
        fs::create_dir(&local_repo).unwrap();
        fs::write(local_repo.join("pixi.toml"), "[workspace]\nname='x'\n").unwrap();
        fs::write(local_repo.join("pixi.lock"), "lock").unwrap();
        let runner_binary = root.path().join("runner");
        fs::write(&runner_binary, "runner").unwrap();
        let cache = root.path().join("missing-cache");
        let output = root.path().join("output");
        let manifest_digest =
            crate::remote::Sha256Digest::from_file(&local_repo.join("pixi.toml")).unwrap();
        let lock_digest =
            crate::remote::Sha256Digest::from_file(&local_repo.join("pixi.lock")).unwrap();
        let fake = FakeRunner::new([
            success(format!(
                "sirius-runner-probe-v3\nLinux\n{}\nglibc 999.0\n570.0\n13.2\n8.0\nyes\n",
                env::consts::ARCH
            )),
            success(format!(
                "ready\n{manifest_digest}\n{lock_digest}\n{}\nclean\n",
                "a".repeat(40)
            )),
            success("miss\n"),
            success("pixi 0.71.0\n"),
        ]);
        let mut client =
            RemoteExecutionClient::with_runner(fake, PixiPackCache::new(&cache), runner_binary)
                .unwrap();
        let mut invocation = RemoteInvocation::new("tpch-sf1", "/srv/sirius", "/datasets");
        invocation.allow_source_difference = true;
        let request = RemoteClientRequest {
            target: SshTarget::new("example.test").unwrap(),
            local_repo,
            local_output: output.clone(),
            run_id: RemoteRunId::new("dry-run").unwrap(),
            invocation,
        };

        let report = client.dry_run(&request, &mut NoopProgress).unwrap();

        assert_eq!(report.local_pack_cache_hit, Some(false));
        assert_eq!(report.local_pixi_version.as_deref(), Some("pixi 0.71.0"));
        assert!(!report.remote_pack_cache_hit);
        assert!(!cache.exists());
        assert!(!output.exists());
        assert_eq!(client.command_runner().commands.len(), 4);
        assert!(client.command_runner().commands[..3].iter().all(|command| {
            command.program == "ssh" && matches!(command.stdin, crate::remote::CommandInput::Null)
        }));
        assert_eq!(client.command_runner().commands[3].program, "pixi");
    }

    fn compatibility(architecture: &str, cuda: &str, glibc: &str) -> RemoteCompatibility {
        RemoteCompatibility {
            os: "Linux".to_owned(),
            architecture: architecture.to_owned(),
            glibc_version: Some(glibc.to_owned()),
            nvidia_driver_version: Some("570.0".to_owned()),
            nvidia_cuda_version: Some(cuda.to_owned()),
            nvidia_compute_capabilities: vec!["8.0".to_owned()],
            flock_available: true,
        }
    }

    fn success(stdout: impl Into<Vec<u8>>) -> ProcessOutput {
        ProcessOutput {
            success: true,
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    fn write_raw_tar(path: &Path, name: &str, contents: &[u8]) {
        let file = File::create(path).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "x".repeat(name.len()), contents)
            .unwrap();
        builder.finish().unwrap();
        drop(builder);

        let mut archive = fs::read(path).unwrap();
        archive[..name.len()].copy_from_slice(name.as_bytes());
        archive[148..156].fill(b' ');
        let checksum = archive[..512]
            .iter()
            .map(|byte| u32::from(*byte))
            .sum::<u32>();
        archive[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
        fs::write(path, archive).unwrap();
    }

    struct FakeRunner {
        responses: VecDeque<ProcessOutput>,
        commands: Vec<CommandSpec>,
    }

    impl FakeRunner {
        fn new(responses: impl IntoIterator<Item = ProcessOutput>) -> Self {
            Self {
                responses: responses.into_iter().collect(),
                commands: Vec::new(),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
            self.commands.push(command.clone());
            let mut output = self
                .responses
                .pop_front()
                .context("fake command runner ran out of responses")?;
            if let CommandOutputTarget::File(path) = &command.stdout {
                fs::write(path, &output.stdout)?;
                output.stdout.clear();
            }
            Ok(output)
        }
    }
}
