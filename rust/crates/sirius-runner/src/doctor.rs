//! Read-only diagnostics for local and SSH benchmark hosts.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    remote::{
        self, CommandInput, CommandOutputTarget, CommandRunner, CommandSpec, ProcessOutput,
        RemoteCompatibility, RemoteRepositoryState, Sha256Digest, SshTarget, SystemCommandRunner,
    },
    remote_execution::{RemoteExecutionTarget, select_execution_target_for_workload},
};

pub const DOCTOR_REPORT_VERSION: u32 = 2;
pub const HOST_ENVIRONMENT_VERSION: u32 = 1;
const PYTHON_RUNTIME_PROBE: &str = concat!(
    "import duckdb, _duckdb, sys; ",
    "print(f'Python {sys.version.split()[0]}; DuckDB {duckdb.__version__}')"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Ready,
    Warning,
    Blocked,
}

impl fmt::Display for DiagnosticStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Ready => "ready",
            Self::Warning => "warning",
            Self::Blocked => "blocked",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueSeverity {
    Warning,
    Error,
}

impl fmt::Display for IssueSeverity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Warning => "warning",
            Self::Error => "error",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticIssue {
    pub severity: IssueSeverity,
    pub code: String,
    pub message: String,
    pub action: String,
}

impl DiagnosticIssue {
    fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            severity: IssueSeverity::Warning,
            code: code.into(),
            message: message.into(),
            action: action.into(),
        }
    }

    fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            severity: IssueSeverity::Error,
            code: code.into(),
            message: message.into(),
            action: action.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryMarker {
    pub path: PathBuf,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryReport {
    pub requested_root: PathBuf,
    pub resolved_root: PathBuf,
    pub valid: bool,
    pub markers: Vec<RepositoryMarker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataRootReport {
    pub path: PathBuf,
    pub exists: bool,
    pub is_directory: bool,
    pub space_probe_path: Option<PathBuf>,
    pub free_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFileReport {
    pub path: PathBuf,
    pub present: bool,
    pub bytes: Option<u64>,
    pub executable: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildArtifactReport {
    pub preset: String,
    pub build_dir: PathBuf,
    pub duckdb: ArtifactFileReport,
    pub sirius_extension: ArtifactFileReport,
    pub usable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Available,
    AvailableViaPixiExec,
    Missing,
}

impl fmt::Display for ToolStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Available => "available",
            Self::AvailableViaPixiExec => "available via pixi exec",
            Self::Missing => "missing",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolReport {
    pub name: String,
    pub required: bool,
    pub status: ToolStatus,
    pub executable: Option<String>,
    pub version: Option<String>,
    pub invocation_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemReport {
    pub os: Option<String>,
    pub architecture: String,
    pub glibc_version: Option<String>,
    pub cpu_model: Option<String>,
    pub logical_cores: Option<u32>,
    pub ram_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NvidiaGpuReport {
    pub index: u32,
    pub name: String,
    pub uuid: String,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NvidiaReport {
    pub available: bool,
    pub driver_version: Option<String>,
    pub cuda_version: Option<String>,
    pub gpus: Vec<NvidiaGpuReport>,
}

/// Read-only host snapshot stored with benchmark results.
///
/// Capture never fails solely because optional system or NVIDIA information is
/// unavailable. Such probe failures are retained as actionable warnings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostEnvironment {
    pub snapshot_version: u32,
    pub captured_at_unix_ms: Option<u64>,
    pub system: SystemReport,
    pub nvidia: NvidiaReport,
    pub warnings: Vec<DiagnosticIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDoctorReport {
    pub report_version: u32,
    pub status: DiagnosticStatus,
    pub cuda_required: bool,
    pub repository: RepositoryReport,
    pub data_root: DataRootReport,
    pub builds: Vec<BuildArtifactReport>,
    pub tools: Vec<ToolReport>,
    pub system: SystemReport,
    pub nvidia: NvidiaReport,
    pub issues: Vec<DiagnosticIssue>,
}

impl fmt::Display for LocalDoctorReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Status: {}", self.status)?;
        writeln!(
            formatter,
            "Workload: {}",
            if self.cuda_required {
                "Sirius/GPU"
            } else {
                "DuckDB/CPU"
            }
        )?;
        writeln!(
            formatter,
            "Repository: {} ({})",
            self.repository.resolved_root.display(),
            if self.repository.valid {
                "valid"
            } else {
                "invalid"
            }
        )?;
        write!(formatter, "Data: {}", self.data_root.path.display())?;
        if let Some(free_bytes) = self.data_root.free_bytes {
            write!(formatter, " ({} free)", format_bytes(free_bytes))?;
        }
        writeln!(formatter)?;

        if self.builds.is_empty() {
            writeln!(formatter, "Builds: none")?;
        } else {
            writeln!(formatter, "Builds:")?;
            for build in &self.builds {
                writeln!(
                    formatter,
                    "  {}: {}",
                    build.preset,
                    if build.usable { "usable" } else { "incomplete" }
                )?;
            }
        }

        writeln!(formatter, "Tools:")?;
        for tool in &self.tools {
            let detail = tool
                .version
                .as_deref()
                .or(tool.invocation_hint.as_deref())
                .unwrap_or("-");
            writeln!(formatter, "  {}: {} ({detail})", tool.name, tool.status)?;
        }

        writeln!(
            formatter,
            "System: {} / {} / {}",
            self.system.os.as_deref().unwrap_or("unknown OS"),
            self.system.architecture,
            self.system
                .glibc_version
                .as_deref()
                .unwrap_or("unknown glibc")
        )?;
        writeln!(
            formatter,
            "CPU: {} / {} logical cores / {} RAM",
            self.system.cpu_model.as_deref().unwrap_or("unknown"),
            self.system
                .logical_cores
                .map_or_else(|| "unknown".to_owned(), |cores| cores.to_string()),
            self.system
                .ram_bytes
                .map_or_else(|| "unknown".to_owned(), format_bytes)
        )?;
        if self.nvidia.available {
            writeln!(
                formatter,
                "NVIDIA: {} GPU(s), driver {}, CUDA {}",
                self.nvidia.gpus.len(),
                self.nvidia.driver_version.as_deref().unwrap_or("unknown"),
                self.nvidia.cuda_version.as_deref().unwrap_or("unknown")
            )?;
        } else {
            writeln!(formatter, "NVIDIA: unavailable")?;
        }

        if !self.issues.is_empty() {
            writeln!(formatter, "Issues:")?;
            for issue in &self.issues {
                writeln!(
                    formatter,
                    "  [{}] {}: {}",
                    issue.severity, issue.code, issue.message
                )?;
                writeln!(formatter, "    Action: {}", issue.action)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDoctorReport {
    pub report_version: u32,
    pub target: String,
    pub status: DiagnosticStatus,
    pub cuda_required: bool,
    pub compatibility: Option<RemoteCompatibility>,
    pub execution_target: Option<RemoteExecutionTarget>,
    pub repository: Option<RemoteRepositoryReport>,
    pub issues: Vec<DiagnosticIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRepositoryReport {
    pub path: PathBuf,
    pub checked: bool,
    pub valid: bool,
    pub state: Option<RemoteRepositoryState>,
}

impl fmt::Display for RemoteDoctorReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "Status: {}", self.status)?;
        writeln!(formatter, "Remote: {}", self.target)?;
        writeln!(
            formatter,
            "Workload: {}",
            if self.cuda_required {
                "Sirius/GPU"
            } else {
                "DuckDB/CPU"
            }
        )?;
        if let Some(compatibility) = &self.compatibility {
            writeln!(
                formatter,
                "System: {} / {} / {}",
                compatibility.os,
                compatibility.architecture,
                compatibility
                    .glibc_version
                    .as_deref()
                    .unwrap_or("unknown glibc")
            )?;
            writeln!(
                formatter,
                "NVIDIA: driver {}, CUDA {}",
                compatibility
                    .nvidia_driver_version
                    .as_deref()
                    .unwrap_or("unavailable"),
                compatibility
                    .nvidia_cuda_version
                    .as_deref()
                    .unwrap_or("unavailable")
            )?;
        }
        if let Some(execution_target) = &self.execution_target {
            writeln!(
                formatter,
                "Pixi target: {} / {}",
                execution_target.target_platform, execution_target.pixi_environment
            )?;
        }
        if let Some(repository) = &self.repository {
            writeln!(
                formatter,
                "Repository: {} ({})",
                repository.path.display(),
                if !repository.checked {
                    "not checked"
                } else if repository.valid {
                    "valid"
                } else {
                    "invalid"
                }
            )?;
            if let Some(state) = &repository.state {
                writeln!(
                    formatter,
                    "Revision: {} ({})",
                    &state.git_commit[..state.git_commit.len().min(12)],
                    if state.git_dirty { "dirty" } else { "clean" }
                )?;
            }
        }
        for issue in &self.issues {
            writeln!(
                formatter,
                "[{}] {}: {}",
                issue.severity, issue.code, issue.message
            )?;
            writeln!(formatter, "  Action: {}", issue.action)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDoctorRequest {
    pub repo_root: PathBuf,
    pub data_root: PathBuf,
    pub build_root: Option<PathBuf>,
}

impl LocalDoctorRequest {
    pub fn new(repo_root: impl Into<PathBuf>, data_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            data_root: data_root.into(),
            build_root: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorProbe {
    Repository,
    DataFilesystem,
    BuildArtifacts,
    RequiredTool,
    System,
    Nvidia,
    RemoteCompatibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorProgressEvent {
    pub probe: DoctorProbe,
    pub message: String,
}

pub trait DoctorProgress {
    fn starting(&mut self, event: DoctorProgressEvent);
}

impl<F> DoctorProgress for F
where
    F: FnMut(DoctorProgressEvent),
{
    fn starting(&mut self, event: DoctorProgressEvent) {
        self(event);
    }
}

#[derive(Debug, Default)]
pub struct NoopDoctorProgress;

impl DoctorProgress for NoopDoctorProgress {
    fn starting(&mut self, _event: DoctorProgressEvent) {}
}

fn progress(sink: &mut impl DoctorProgress, probe: DoctorProbe, message: impl Into<String>) {
    sink.starting(DoctorProgressEvent {
        probe,
        message: message.into(),
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorMetadata {
    pub is_file: bool,
    pub is_directory: bool,
    pub bytes: u64,
    pub executable: Option<bool>,
}

/// Read-only host operations, separated for deterministic diagnostics tests.
pub trait DoctorBackend: CommandRunner {
    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf>;
    fn metadata(&mut self, path: &Path) -> io::Result<DoctorMetadata>;
    fn read_directory(&mut self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn read_text(&mut self, path: &Path) -> io::Result<String>;
    fn architecture(&self) -> String;
    fn available_parallelism(&self) -> Option<u32>;
}

#[derive(Debug, Default)]
pub struct SystemDoctorBackend {
    commands: SystemCommandRunner,
}

impl CommandRunner for SystemDoctorBackend {
    fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
        self.commands.run(command)
    }
}

impl DoctorBackend for SystemDoctorBackend {
    fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(path)
    }

    fn metadata(&mut self, path: &Path) -> io::Result<DoctorMetadata> {
        let metadata = fs::metadata(path)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata
                .is_file()
                .then(|| metadata.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let executable = None;
        Ok(DoctorMetadata {
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
            bytes: metadata.len(),
            executable,
        })
    }

    fn read_directory(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }

    fn read_text(&mut self, path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    fn architecture(&self) -> String {
        std::env::consts::ARCH.to_owned()
    }

    fn available_parallelism(&self) -> Option<u32> {
        std::thread::available_parallelism()
            .ok()
            .and_then(|cores| u32::try_from(cores.get()).ok())
    }
}

pub fn diagnose_local_system(
    request: &LocalDoctorRequest,
    progress_sink: &mut impl DoctorProgress,
) -> LocalDoctorReport {
    diagnose_local_system_for_workload(request, false, progress_sink)
}

pub fn diagnose_local_system_for_workload(
    request: &LocalDoctorRequest,
    cuda_required: bool,
    progress_sink: &mut impl DoctorProgress,
) -> LocalDoctorReport {
    diagnose_local_for_workload(
        request,
        cuda_required,
        &mut SystemDoctorBackend::default(),
        progress_sink,
    )
}

/// Captures only host properties needed to interpret benchmark results.
///
/// This does not inspect repository state, datasets, builds, or development
/// tools. Every external command emits a progress event before it starts.
pub fn capture_host_environment_system(progress_sink: &mut impl DoctorProgress) -> HostEnvironment {
    capture_host_environment(&mut SystemDoctorBackend::default(), progress_sink)
}

/// Testable form of [`capture_host_environment_system`] using a read-only
/// backend supplied by the caller.
pub fn capture_host_environment(
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
) -> HostEnvironment {
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
    let mut warnings = Vec::new();
    let system = inspect_system(backend, progress_sink, &mut warnings);
    let nvidia = inspect_nvidia(backend, progress_sink, &mut warnings);
    debug_assert!(
        warnings
            .iter()
            .all(|issue| issue.severity == IssueSeverity::Warning),
        "host snapshot probes should degrade to warnings"
    );
    HostEnvironment {
        snapshot_version: HOST_ENVIRONMENT_VERSION,
        captured_at_unix_ms,
        system,
        nvidia,
        warnings,
    }
}

pub fn diagnose_local(
    request: &LocalDoctorRequest,
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
) -> LocalDoctorReport {
    diagnose_local_for_workload(request, false, backend, progress_sink)
}

pub fn diagnose_local_for_workload(
    request: &LocalDoctorRequest,
    cuda_required: bool,
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
) -> LocalDoctorReport {
    let mut issues = Vec::new();

    progress(
        progress_sink,
        DoctorProbe::Repository,
        "Checking repository root",
    );
    let repository = inspect_repository(request, backend, &mut issues);
    let repo_root = &repository.resolved_root;
    if cuda_required {
        progress(
            progress_sink,
            DoctorProbe::Repository,
            "Checking Sirius source submodules",
        );
        inspect_required_submodules(repo_root, backend, &mut issues);
    }

    let data_path = resolve_under(repo_root, &request.data_root);
    progress(
        progress_sink,
        DoctorProbe::DataFilesystem,
        format!("Checking free space for {}", data_path.display()),
    );
    let data_root = inspect_data_root(data_path, backend, &mut issues);

    let build_root = request
        .build_root
        .as_ref()
        .map(|path| resolve_under(repo_root, path))
        .unwrap_or_else(|| repo_root.join("build"));
    progress(
        progress_sink,
        DoctorProbe::BuildArtifacts,
        format!("Inspecting build artifacts in {}", build_root.display()),
    );
    let builds = inspect_builds(&build_root, backend, &mut issues);

    let tools = inspect_tools(repo_root, backend, progress_sink, &mut issues);
    let system = inspect_system(backend, progress_sink, &mut issues);
    let nvidia = inspect_nvidia(backend, progress_sink, &mut issues);
    if cuda_required
        && (!nvidia.available
            || nvidia.gpus.is_empty()
            || nvidia.driver_version.is_none()
            || nvidia.cuda_version.is_none())
    {
        issues.push(DiagnosticIssue::error(
            "nvidia-required",
            "the requested Sirius workload needs a usable NVIDIA GPU, driver, and CUDA compatibility",
            "Install or expose a supported NVIDIA GPU and driver, or check a DuckDB-only workload with --engine duckdb.",
        ));
    }
    let status = status_from_issues(&issues);

    LocalDoctorReport {
        report_version: DOCTOR_REPORT_VERSION,
        status,
        cuda_required,
        repository,
        data_root,
        builds,
        tools,
        system,
        nvidia,
        issues,
    }
}

pub fn diagnose_remote(
    target: &SshTarget,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> RemoteDoctorReport {
    diagnose_remote_inner(target, None, None, true, false, runner, progress_sink)
}

pub fn diagnose_remote_with_repository(
    target: &SshTarget,
    local_repository: &Path,
    remote_repository: &Path,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> RemoteDoctorReport {
    diagnose_remote_with_repository_for_workload(
        target,
        local_repository,
        remote_repository,
        true,
        false,
        runner,
        progress_sink,
    )
}

pub fn diagnose_remote_with_repository_for_workload(
    target: &SshTarget,
    local_repository: &Path,
    remote_repository: &Path,
    cuda_required: bool,
    allow_source_difference: bool,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> RemoteDoctorReport {
    diagnose_remote_inner(
        target,
        Some(local_repository),
        Some(remote_repository),
        cuda_required,
        allow_source_difference,
        runner,
        progress_sink,
    )
}

fn diagnose_remote_inner(
    target: &SshTarget,
    local_repository: Option<&Path>,
    remote_repository: Option<&Path>,
    cuda_required: bool,
    allow_source_difference: bool,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> RemoteDoctorReport {
    let mut issues = Vec::new();
    if local_repository.is_some() {
        progress(
            progress_sink,
            DoctorProbe::RequiredTool,
            "Checking local Pixi for remote environment packing",
        );
        if let Err(error) = probe_local_pixi(runner) {
            issues.push(DiagnosticIssue::error(
                "local-pixi-unavailable",
                format!("{error:#}"),
                "Install Pixi and ensure `pixi --version` succeeds before remote execution.",
            ));
        }
    }
    progress(
        progress_sink,
        DoctorProbe::RemoteCompatibility,
        format!("Probing remote compatibility on {}", target.as_str()),
    );
    let compatibility = match remote::probe_compatibility(runner, target) {
        Ok(compatibility) => Some(compatibility),
        Err(error) => {
            issues.push(DiagnosticIssue::error(
                "remote-probe-failed",
                format!("{error:#}"),
                format!(
                    "Verify `ssh -o BatchMode=yes {}` succeeds without prompting.",
                    target.as_str()
                ),
            ));
            None
        }
    };
    let execution_target = compatibility.as_ref().and_then(|compatibility| {
        progress(
            progress_sink,
            DoctorProbe::RemoteCompatibility,
            "Checking uploaded-runner, glibc, and CUDA compatibility",
        );
        match select_execution_target_for_workload(compatibility, cuda_required) {
            Ok(target) => Some(target),
            Err(error) => {
                issues.push(DiagnosticIssue::error(
                    "remote-execution-incompatible",
                    format!("{error:#}"),
                    if cuda_required {
                        "Use a same-architecture Linux host whose glibc can run this binary and whose default GPU and NVIDIA driver support the selected CUDA profile."
                    } else {
                        "Use a same-architecture Linux host whose glibc can run this binary."
                    },
                ));
                None
            }
        }
    });
    let repository = remote_repository.map(|path| {
        if compatibility.is_none() {
            return RemoteRepositoryReport {
                path: path.to_owned(),
                checked: false,
                valid: false,
                state: None,
            };
        }

        progress(
            progress_sink,
            DoctorProbe::Repository,
            format!(
                "Checking remote Sirius checkout {} on {}",
                path.display(),
                target.as_str()
            ),
        );
        match remote::probe_remote_repository(runner, target, path) {
            Ok(state) => {
                let mut valid = true;
                if let Some(local_repository) = local_repository {
                    progress(
                        progress_sink,
                        DoctorProbe::Repository,
                        "Comparing local and remote Pixi inputs",
                    );
                    match local_repository_state(local_repository) {
                        Ok(local)
                            if local.manifest_sha256 == state.manifest_sha256
                                && local.lock_sha256 == state.lock_sha256 =>
                        {
                            let source_differs = local.git_dirty
                                || state.git_dirty
                                || local.git_commit != state.git_commit;
                            if source_differs && !allow_source_difference {
                                valid = false;
                                issues.push(DiagnosticIssue::error(
                                    "remote-source-differs",
                                    format!(
                                        "local/remote sources are not the same clean commit (local {}{}, remote {}{})",
                                        local.git_commit,
                                        if local.git_dirty { ", dirty" } else { "" },
                                        state.git_commit,
                                        if state.git_dirty { ", dirty" } else { "" }
                                    ),
                                    "Synchronize and clean both checkouts, or pass --allow-remote-source-difference.",
                                ));
                            } else if source_differs {
                                issues.push(DiagnosticIssue::warning(
                                    "remote-source-difference-allowed",
                                    "local and remote source revisions differ under an explicit override",
                                    "Use the recorded remote commit and dirty status when interpreting results.",
                                ));
                            }
                        }
                        Ok(_) => {
                            valid = false;
                            issues.push(DiagnosticIssue::error(
                                "remote-pixi-inputs-differ",
                                "local and remote pixi.toml/pixi.lock checksums differ",
                                "Synchronize the remote checkout with the local checkout before running.",
                            ));
                        }
                        Err(error) => {
                            valid = false;
                            issues.push(DiagnosticIssue::error(
                                "local-pixi-inputs-invalid",
                                format!("{error:#}"),
                                "Pass `--repo-root` pointing to the local Sirius checkout.",
                            ));
                        }
                    }
                }
                RemoteRepositoryReport {
                    path: path.to_owned(),
                    checked: true,
                    valid,
                    state: Some(state),
                }
            }
            Err(error) => {
                issues.push(DiagnosticIssue::error(
                    "remote-repository-invalid",
                    format!("{error:#}"),
                    "Pass `--remote-repo` pointing to a complete Sirius checkout with initialized submodules.",
                ));
                RemoteRepositoryReport {
                    path: path.to_owned(),
                    checked: true,
                    valid: false,
                    state: None,
                }
            }
        }
    });

    RemoteDoctorReport {
        report_version: DOCTOR_REPORT_VERSION,
        target: target.as_str().to_owned(),
        status: status_from_issues(&issues),
        cuda_required,
        compatibility,
        execution_target,
        repository,
        issues,
    }
}

fn probe_local_pixi(runner: &mut impl CommandRunner) -> anyhow::Result<()> {
    let command = CommandSpec {
        program: OsString::from("pixi"),
        args: vec![OsString::from("--version")],
        current_dir: None,
        stdin: CommandInput::Null,
        stdout: CommandOutputTarget::Capture,
    };
    let output = runner.run(&command).context("running `pixi --version`")?;
    ensure!(
        output.success,
        "`pixi --version` failed with exit code {}",
        output
            .code
            .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
    );
    Ok(())
}

fn local_repository_state(repository: &Path) -> anyhow::Result<RemoteRepositoryState> {
    let manifest = repository.join("pixi.toml");
    let lock = repository.join("pixi.lock");
    ensure!(
        manifest.is_file() && lock.is_file(),
        "{} does not contain pixi.toml and pixi.lock",
        repository.display()
    );
    Ok(RemoteRepositoryState {
        manifest_sha256: Sha256Digest::from_file(&manifest)?,
        lock_sha256: Sha256Digest::from_file(&lock)?,
        git_commit: crate::repository::git_output(repository, &["rev-parse", "HEAD"])?,
        git_dirty: crate::repository::is_dirty(repository)?,
    })
}

fn inspect_repository(
    request: &LocalDoctorRequest,
    backend: &mut impl DoctorBackend,
    issues: &mut Vec<DiagnosticIssue>,
) -> RepositoryReport {
    let resolved_root = match backend.canonicalize(&request.repo_root) {
        Ok(path) => path,
        Err(error) => {
            issues.push(DiagnosticIssue::error(
                "repo-root-unreadable",
                format!(
                    "cannot resolve repository root {}: {error}",
                    request.repo_root.display()
                ),
                "Pass `--repo-root` pointing to a Sirius checkout or worktree.",
            ));
            request.repo_root.clone()
        }
    };

    let required_markers = [
        (Path::new(".git"), MarkerKind::FileOrDirectory),
        (Path::new("pixi.toml"), MarkerKind::File),
        (Path::new("CMakeLists.txt"), MarkerKind::File),
        (
            Path::new("rust/crates/sirius-runner/Cargo.toml"),
            MarkerKind::File,
        ),
    ];
    let markers = required_markers
        .iter()
        .map(|(relative, kind)| {
            let path = resolved_root.join(relative);
            let present = backend
                .metadata(&path)
                .is_ok_and(|metadata| kind.matches(metadata));
            RepositoryMarker { path, present }
        })
        .collect::<Vec<_>>();
    let valid = markers.iter().all(|marker| marker.present);
    if !valid {
        let missing = markers
            .iter()
            .filter(|marker| !marker.present)
            .map(|marker| marker.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        issues.push(DiagnosticIssue::error(
            "repo-root-invalid",
            format!("repository root is missing required markers: {missing}"),
            "Pass the root of the Sirius checkout, not a nested Pixi project.",
        ));
    }

    RepositoryReport {
        requested_root: request.repo_root.clone(),
        resolved_root,
        valid,
        markers,
    }
}

fn inspect_required_submodules(
    repo_root: &Path,
    backend: &mut impl DoctorBackend,
    issues: &mut Vec<DiagnosticIssue>,
) {
    for name in ["duckdb", "cucascade", "substrait"] {
        let marker = repo_root.join(name).join(".git");
        let initialized = backend
            .metadata(&marker)
            .is_ok_and(|metadata| metadata.is_file || metadata.is_directory);
        if !initialized {
            issues.push(DiagnosticIssue::error(
                "submodule-uninitialized",
                format!("required Sirius submodule `{name}` is not initialized"),
                "Run `git submodule update --init --recursive` in the Sirius checkout.",
            ));
        }
    }
}

#[derive(Clone, Copy)]
enum MarkerKind {
    File,
    FileOrDirectory,
}

impl MarkerKind {
    fn matches(self, metadata: DoctorMetadata) -> bool {
        match self {
            Self::File => metadata.is_file,
            Self::FileOrDirectory => metadata.is_file || metadata.is_directory,
        }
    }
}

fn inspect_data_root(
    path: PathBuf,
    backend: &mut impl DoctorBackend,
    issues: &mut Vec<DiagnosticIssue>,
) -> DataRootReport {
    let metadata = backend.metadata(&path).ok();
    let exists = metadata.is_some();
    let is_directory = metadata.is_some_and(|metadata| metadata.is_directory);
    if exists && !is_directory {
        issues.push(DiagnosticIssue::error(
            "data-root-not-directory",
            format!("data root {} is not a directory", path.display()),
            "Choose a directory with `--data-root`.",
        ));
    } else if !exists {
        issues.push(DiagnosticIssue::warning(
            "data-root-missing",
            format!("data root {} does not exist", path.display()),
            "Create the directory or let the first benchmark run create it after reviewing its plan.",
        ));
    }

    let space_probe_path = nearest_existing_directory(&path, backend);
    let free_bytes = space_probe_path
        .as_ref()
        .and_then(|probe_path| probe_free_bytes(probe_path, backend).ok());
    if space_probe_path.is_none() || free_bytes.is_none() {
        issues.push(DiagnosticIssue::warning(
            "data-free-space-unknown",
            format!("could not determine free space for {}", path.display()),
            "Run `df -Pk` for the data filesystem and confirm it has enough capacity.",
        ));
    }

    DataRootReport {
        path,
        exists,
        is_directory,
        space_probe_path,
        free_bytes,
    }
}

fn nearest_existing_directory(path: &Path, backend: &mut impl DoctorBackend) -> Option<PathBuf> {
    let mut candidate = Some(path);
    while let Some(path) = candidate {
        if backend
            .metadata(path)
            .is_ok_and(|metadata| metadata.is_directory)
        {
            return Some(path.to_owned());
        }
        candidate = path.parent();
    }
    None
}

fn probe_free_bytes(path: &Path, runner: &mut impl CommandRunner) -> anyhow::Result<u64> {
    let output = runner.run(&command(
        "df",
        [OsString::from("-Pk"), path.as_os_str().to_owned()],
    ))?;
    if !output.success {
        anyhow::bail!("df failed");
    }
    parse_df_free_bytes(&String::from_utf8_lossy(&output.stdout))
}

fn parse_df_free_bytes(output: &str) -> anyhow::Result<u64> {
    let line = output
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .context("df returned no filesystem row")?;
    let blocks = line
        .split_whitespace()
        .nth(3)
        .context("df filesystem row omitted available blocks")?
        .parse::<u64>()
        .context("df reported an invalid available-block count")?;
    blocks
        .checked_mul(1024)
        .context("df available-byte count overflowed u64")
}

fn inspect_builds(
    build_root: &Path,
    backend: &mut impl DoctorBackend,
    issues: &mut Vec<DiagnosticIssue>,
) -> Vec<BuildArtifactReport> {
    let mut directories = backend.read_directory(build_root).unwrap_or_default();
    directories.sort();
    let mut builds = Vec::new();
    for build_dir in directories {
        if !backend
            .metadata(&build_dir)
            .is_ok_and(|metadata| metadata.is_directory)
        {
            continue;
        }
        let Some(preset) = build_dir.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let duckdb = artifact_file(&build_dir.join("duckdb"), true, backend);
        let sirius_extension = artifact_file(
            &build_dir.join("extension/sirius/sirius.duckdb_extension"),
            false,
            backend,
        );
        let usable =
            duckdb.present && duckdb.executable.unwrap_or(false) && sirius_extension.present;
        builds.push(BuildArtifactReport {
            preset: preset.to_owned(),
            build_dir,
            duckdb,
            sirius_extension,
            usable,
        });
    }
    if !builds.iter().any(|build| build.usable) {
        issues.push(DiagnosticIssue::warning(
            "build-artifacts-missing",
            format!(
                "no usable DuckDB and Sirius extension pair found in {}",
                build_root.display()
            ),
            "Run `pixi run make release`, or select an explicit prebuilt artifact pair.",
        ));
    }
    builds
}

fn artifact_file(
    path: &Path,
    requires_executable: bool,
    backend: &mut impl DoctorBackend,
) -> ArtifactFileReport {
    let metadata = backend
        .metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file);
    ArtifactFileReport {
        path: path.to_owned(),
        present: metadata.is_some(),
        bytes: metadata.map(|metadata| metadata.bytes),
        executable: requires_executable
            .then(|| metadata.and_then(|metadata| metadata.executable))
            .flatten(),
    }
}

fn inspect_tools(
    repo_root: &Path,
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
    issues: &mut Vec<DiagnosticIssue>,
) -> Vec<ToolReport> {
    inspect_tools_with_python(
        &crate::worker::python_executable(),
        repo_root,
        backend,
        progress_sink,
        issues,
    )
}

fn inspect_tools_with_python(
    python: &str,
    repo_root: &Path,
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
    issues: &mut Vec<DiagnosticIssue>,
) -> Vec<ToolReport> {
    let mut tools = Vec::new();
    for (name, executable, arguments, action) in [
        (
            "git",
            "git",
            &["--version"][..],
            "Install Git and ensure `git` is on PATH.",
        ),
        (
            "pixi",
            "pixi",
            &["--version"][..],
            "Install Pixi and ensure `pixi` is on PATH.",
        ),
        (
            "make",
            "make",
            &["--version"][..],
            "Install GNU Make and ensure `make` is on PATH.",
        ),
    ] {
        let report = probe_tool(name, &[executable], arguments, true, backend, progress_sink);
        if report.status == ToolStatus::Missing {
            issues.push(DiagnosticIssue::error(
                format!("tool-{name}-missing"),
                format!("required tool `{name}` is unavailable"),
                action,
            ));
        }
        tools.push(report);
    }

    let python = probe_tool_from(
        "python",
        &[python],
        &["-c", PYTHON_RUNTIME_PROBE],
        true,
        Some(repo_root),
        backend,
        progress_sink,
    );
    if python.status == ToolStatus::Missing {
        issues.push(DiagnosticIssue::error(
            "python-worker-runtime-unavailable",
            "the benchmark worker Python cannot import both `duckdb` and `_duckdb`",
            "Install the DuckDB Python package in the selected environment, or set `SIRIUS_RUNNER_PYTHON` to the exact Python executable the runner should use.",
        ));
    }
    tools.push(python);

    let ssh = probe_tool("ssh", &["ssh"], &["-V"], false, backend, progress_sink);
    if ssh.status == ToolStatus::Missing {
        issues.push(DiagnosticIssue::warning(
            "tool-ssh-missing",
            "SSH is unavailable, so remote benchmark execution cannot be used",
            "Install an OpenSSH client and ensure `ssh` is on PATH before using `--remote`.",
        ));
    }
    tools.push(ssh);

    let mut pixi_pack = probe_tool(
        "pixi-pack",
        &["pixi-pack"],
        &["--version"],
        false,
        backend,
        progress_sink,
    );
    if pixi_pack.status == ToolStatus::Missing
        && tools
            .iter()
            .any(|tool| tool.name == "pixi" && tool.status == ToolStatus::Available)
    {
        pixi_pack.status = ToolStatus::AvailableViaPixiExec;
        pixi_pack.executable = Some("pixi".to_owned());
        pixi_pack.invocation_hint = Some("pixi exec --spec pixi-pack -- pixi-pack".to_owned());
    } else if pixi_pack.status == ToolStatus::Missing {
        issues.push(DiagnosticIssue::warning(
            "tool-pixi-pack-missing",
            "pixi-pack is unavailable for remote environment packaging",
            "Install Pixi; the runner can invoke pixi-pack through `pixi exec --spec pixi-pack`.",
        ));
    }
    tools.push(pixi_pack);
    tools
}

fn probe_tool(
    name: &str,
    candidates: &[&str],
    arguments: &[&str],
    required: bool,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> ToolReport {
    probe_tool_from(
        name,
        candidates,
        arguments,
        required,
        None,
        runner,
        progress_sink,
    )
}

#[allow(clippy::too_many_arguments)]
fn probe_tool_from(
    name: &str,
    candidates: &[&str],
    arguments: &[&str],
    required: bool,
    current_dir: Option<&Path>,
    runner: &mut impl CommandRunner,
    progress_sink: &mut impl DoctorProgress,
) -> ToolReport {
    for candidate in candidates {
        progress(
            progress_sink,
            DoctorProbe::RequiredTool,
            format!("Checking {name} via `{candidate}`"),
        );
        let mut command = command(candidate, arguments.iter().copied().map(OsString::from));
        command.current_dir = current_dir.map(Path::to_owned);
        let output = runner.run(&command);
        if let Ok(output) = output
            && output.success
        {
            return ToolReport {
                name: name.to_owned(),
                required,
                status: ToolStatus::Available,
                executable: Some((*candidate).to_owned()),
                version: first_output_line(&output),
                invocation_hint: None,
            };
        }
    }
    ToolReport {
        name: name.to_owned(),
        required,
        status: ToolStatus::Missing,
        executable: None,
        version: None,
        invocation_hint: None,
    }
}

fn inspect_system(
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
    issues: &mut Vec<DiagnosticIssue>,
) -> SystemReport {
    progress(
        progress_sink,
        DoctorProbe::System,
        "Reading CPU, memory, and operating-system information",
    );
    let cpu_info = backend.read_text(Path::new("/proc/cpuinfo")).ok();
    let cpu_model = cpu_info.as_deref().and_then(parse_cpu_model);
    let logical_cores = cpu_info
        .as_deref()
        .and_then(parse_cpu_cores)
        .or_else(|| backend.available_parallelism());
    let ram_bytes = backend
        .read_text(Path::new("/proc/meminfo"))
        .ok()
        .and_then(|contents| parse_ram_bytes(&contents));
    let os = backend
        .read_text(Path::new("/etc/os-release"))
        .ok()
        .and_then(|contents| parse_os_release(&contents));

    progress(
        progress_sink,
        DoctorProbe::System,
        "Checking glibc compatibility",
    );
    let glibc_version = backend
        .run(&command("getconf", [OsString::from("GNU_LIBC_VERSION")]))
        .ok()
        .filter(|output| output.success)
        .and_then(|output| first_output_line(&output));

    if cpu_model.is_none() || logical_cores.is_none() {
        issues.push(DiagnosticIssue::warning(
            "cpu-info-unavailable",
            "CPU model or logical core count could not be determined",
            "Check that `/proc/cpuinfo` is readable on the benchmark host.",
        ));
    }
    if ram_bytes.is_none() {
        issues.push(DiagnosticIssue::warning(
            "ram-info-unavailable",
            "total RAM could not be determined",
            "Check that `/proc/meminfo` is readable on the benchmark host.",
        ));
    }
    if os.is_none() {
        issues.push(DiagnosticIssue::warning(
            "os-info-unavailable",
            "operating-system release could not be determined",
            "Check that `/etc/os-release` is readable.",
        ));
    }
    if glibc_version.is_none() {
        issues.push(DiagnosticIssue::warning(
            "glibc-info-unavailable",
            "glibc version could not be determined",
            "Install `getconf` or verify `getconf GNU_LIBC_VERSION` manually.",
        ));
    }

    SystemReport {
        os,
        architecture: backend.architecture(),
        glibc_version,
        cpu_model,
        logical_cores,
        ram_bytes,
    }
}

fn inspect_nvidia(
    backend: &mut impl DoctorBackend,
    progress_sink: &mut impl DoctorProgress,
    issues: &mut Vec<DiagnosticIssue>,
) -> NvidiaReport {
    progress(
        progress_sink,
        DoctorProbe::Nvidia,
        "Querying NVIDIA GPUs and driver",
    );
    let query = command(
        "nvidia-smi",
        [
            OsString::from("--query-gpu=index,name,uuid,memory.total,driver_version"),
            OsString::from("--format=csv,noheader,nounits"),
        ],
    );
    let output = match backend.run(&query) {
        Ok(output) if output.success => output,
        Ok(output) => {
            issues.push(DiagnosticIssue::warning(
                "nvidia-smi-failed",
                format!(
                    "`nvidia-smi` exited with {}",
                    output
                        .code
                        .map_or_else(|| "unknown status".to_owned(), |code| code.to_string())
                ),
                "Install or repair the NVIDIA driver before running GPU benchmarks.",
            ));
            return NvidiaReport {
                available: false,
                driver_version: None,
                cuda_version: None,
                gpus: Vec::new(),
            };
        }
        Err(error) => {
            issues.push(DiagnosticIssue::warning(
                "nvidia-smi-unavailable",
                format!("NVIDIA hardware probe failed: {error:#}"),
                "Install the NVIDIA driver and ensure `nvidia-smi` is on PATH for GPU benchmarks.",
            ));
            return NvidiaReport {
                available: false,
                driver_version: None,
                cuda_version: None,
                gpus: Vec::new(),
            };
        }
    };

    let (gpus, driver_version, malformed_rows) =
        parse_nvidia_query(&String::from_utf8_lossy(&output.stdout));
    if malformed_rows > 0 {
        issues.push(DiagnosticIssue::warning(
            "nvidia-output-malformed",
            format!("{malformed_rows} `nvidia-smi` GPU row(s) could not be parsed"),
            "Run the reported `nvidia-smi --query-gpu=...` command manually and inspect its output.",
        ));
    }
    if gpus.is_empty() {
        issues.push(DiagnosticIssue::warning(
            "nvidia-gpu-missing",
            "`nvidia-smi` reported no usable GPU",
            "Run GPU benchmarks on a host with a visible NVIDIA GPU.",
        ));
    }

    progress(
        progress_sink,
        DoctorProbe::Nvidia,
        "Querying NVIDIA CUDA compatibility",
    );
    let cuda_version = backend
        .run(&command("nvidia-smi", std::iter::empty::<OsString>()))
        .ok()
        .filter(|output| output.success)
        .and_then(|output| parse_cuda_version(&String::from_utf8_lossy(&output.stdout)));
    if cuda_version.is_none() {
        issues.push(DiagnosticIssue::warning(
            "nvidia-cuda-version-unknown",
            "NVIDIA driver CUDA compatibility version could not be determined",
            "Run `nvidia-smi` and verify its `CUDA Version` header.",
        ));
    }

    NvidiaReport {
        available: !gpus.is_empty(),
        driver_version,
        cuda_version,
        gpus,
    }
}

fn parse_nvidia_query(output: &str) -> (Vec<NvidiaGpuReport>, Option<String>, usize) {
    let mut gpus = Vec::new();
    let mut driver_version = None;
    let mut malformed = 0;
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() != 5 {
            malformed += 1;
            continue;
        }
        let Some(index) = fields[0].parse::<u32>().ok() else {
            malformed += 1;
            continue;
        };
        let Some(memory_bytes) = fields[3]
            .parse::<u64>()
            .ok()
            .and_then(|mib| mib.checked_mul(1024 * 1024))
        else {
            malformed += 1;
            continue;
        };
        driver_version.get_or_insert_with(|| fields[4].to_owned());
        gpus.push(NvidiaGpuReport {
            index,
            name: fields[1].to_owned(),
            uuid: fields[2].to_owned(),
            memory_bytes,
        });
    }
    (gpus, driver_version, malformed)
}

fn parse_cuda_version(output: &str) -> Option<String> {
    let suffix = output.split_once("CUDA Version:")?.1.trim_start();
    suffix
        .split(|character: char| character.is_whitespace() || character == '|')
        .next()
        .filter(|version| !version.is_empty() && *version != "N/A")
        .map(str::to_owned)
}

fn parse_cpu_model(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        matches!(name.trim(), "model name" | "Hardware" | "Processor")
            .then(|| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn parse_cpu_cores(contents: &str) -> Option<u32> {
    let count = contents
        .lines()
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(name, _)| name.trim() == "processor")
        })
        .count();
    (count > 0).then(|| u32::try_from(count).ok()).flatten()
}

fn parse_ram_bytes(contents: &str) -> Option<u64> {
    let line = contents
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?;
    let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    kib.checked_mul(1024)
}

fn parse_os_release(contents: &str) -> Option<String> {
    let values = contents
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            Some((name, value.trim_matches('"')))
        })
        .collect::<BTreeMap<_, _>>();
    values
        .get("PRETTY_NAME")
        .or_else(|| values.get("NAME"))
        .map(|value| (*value).to_owned())
}

fn first_output_line(output: &ProcessOutput) -> Option<String> {
    [&output.stdout, &output.stderr]
        .into_iter()
        .flat_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

fn command(
    program: impl Into<OsString>,
    arguments: impl IntoIterator<Item = OsString>,
) -> CommandSpec {
    CommandSpec {
        program: program.into(),
        args: arguments.into_iter().collect(),
        current_dir: None,
        stdin: CommandInput::Null,
        stdout: CommandOutputTarget::Capture,
    }
}

fn resolve_under(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn status_from_issues(issues: &[DiagnosticIssue]) -> DiagnosticStatus {
    if issues
        .iter()
        .any(|issue| issue.severity == IssueSeverity::Error)
    {
        DiagnosticStatus::Blocked
    } else if issues.is_empty() {
        DiagnosticStatus::Ready
    } else {
        DiagnosticStatus::Warning
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, VecDeque},
        fs,
        sync::{Arc, Mutex},
    };

    use anyhow::anyhow;
    use tempfile::TempDir;

    use super::*;

    #[derive(Debug)]
    struct FakeBackend {
        commands: BTreeMap<String, VecDeque<anyhow::Result<ProcessOutput>>>,
        text: BTreeMap<PathBuf, io::Result<String>>,
        events: Arc<Mutex<Vec<String>>>,
        architecture: String,
        cores: Option<u32>,
    }

    impl FakeBackend {
        fn new(events: Arc<Mutex<Vec<String>>>) -> Self {
            Self {
                commands: BTreeMap::new(),
                text: BTreeMap::new(),
                events,
                architecture: "x86_64".to_owned(),
                cores: Some(16),
            }
        }

        fn command_output(
            &mut self,
            program: &str,
            arguments: &[&str],
            output: anyhow::Result<ProcessOutput>,
        ) {
            self.commands
                .entry(command_key(
                    program,
                    arguments.iter().copied().map(OsString::from),
                ))
                .or_default()
                .push_back(output);
        }

        fn text(&mut self, path: &str, value: &str) {
            self.text.insert(PathBuf::from(path), Ok(value.to_owned()));
        }
    }

    impl CommandRunner for FakeBackend {
        fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
            let key = command_key(&command.program.to_string_lossy(), command.args.clone());
            self.events.lock().unwrap().push(format!("call:{key}"));
            self.commands
                .get_mut(&key)
                .and_then(VecDeque::pop_front)
                .unwrap_or_else(|| Err(anyhow!("unexpected command {key}")))
        }
    }

    impl DoctorBackend for FakeBackend {
        fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
            fs::canonicalize(path)
        }

        fn metadata(&mut self, path: &Path) -> io::Result<DoctorMetadata> {
            let metadata = fs::metadata(path)?;
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata
                    .is_file()
                    .then(|| metadata.permissions().mode() & 0o111 != 0)
            };
            #[cfg(not(unix))]
            let executable = None;
            Ok(DoctorMetadata {
                is_file: metadata.is_file(),
                is_directory: metadata.is_dir(),
                bytes: metadata.len(),
                executable,
            })
        }

        fn read_directory(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
            fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        }

        fn read_text(&mut self, path: &Path) -> io::Result<String> {
            match self.text.remove(path) {
                Some(result) => result,
                None => fs::read_to_string(path),
            }
        }

        fn architecture(&self) -> String {
            self.architecture.clone()
        }

        fn available_parallelism(&self) -> Option<u32> {
            self.cores
        }
    }

    fn successful(stdout: impl Into<Vec<u8>>) -> ProcessOutput {
        ProcessOutput {
            success: true,
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    fn failed(code: i32, stderr: &str) -> ProcessOutput {
        ProcessOutput {
            success: false,
            code: Some(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn fixture() -> TempDir {
        let temp = TempDir::new().unwrap();
        for relative in [
            "rust/crates/sirius-runner",
            "data",
            "build/release/extension/sirius",
        ] {
            fs::create_dir_all(temp.path().join(relative)).unwrap();
        }
        for relative in [
            "pixi.toml",
            "pixi.lock",
            "CMakeLists.txt",
            "rust/crates/sirius-runner/Cargo.toml",
            "build/release/duckdb",
            "build/release/extension/sirius/sirius.duckdb_extension",
        ] {
            fs::write(temp.path().join(relative), b"fixture").unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = temp.path().join("build/release/duckdb");
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
        for arguments in [
            &["init", "--quiet"][..],
            &["config", "user.email", "runner@example.test"][..],
            &["config", "user.name", "Runner Tests"][..],
            &["add", "."][..],
            &["commit", "--quiet", "-m", "fixture"][..],
        ] {
            let status = std::process::Command::new("git")
                .current_dir(temp.path())
                .args(arguments)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "Git fixture command failed: {arguments:?}"
            );
        }
        temp
    }

    fn ready_backend(events: Arc<Mutex<Vec<String>>>, data_path: &Path) -> FakeBackend {
        let mut backend = FakeBackend::new(events);
        backend.command_output(
            "df",
            &["-Pk", &data_path.display().to_string()],
            Ok(successful(
                b"Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 1000 100 900 10% /x\n".to_vec(),
            )),
        );
        for (program, arguments, version) in [
            ("git", &["--version"][..], "git version 2.50"),
            ("pixi", &["--version"][..], "pixi 0.49"),
            ("make", &["--version"][..], "GNU Make 4.4"),
        ] {
            backend.command_output(program, arguments, Ok(successful(format!("{version}\n"))));
        }
        backend.command_output(
            "python",
            &["-c", PYTHON_RUNTIME_PROBE],
            Ok(successful("Python 3.13; DuckDB 1.4.0\n")),
        );
        backend.command_output(
            "ssh",
            &["-V"],
            Ok(ProcessOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: b"OpenSSH_9.9\n".to_vec(),
            }),
        );
        backend.command_output("pixi-pack", &["--version"], Err(anyhow!("not installed")));
        backend.command_output(
            "getconf",
            &["GNU_LIBC_VERSION"],
            Ok(successful(b"glibc 2.39\n".to_vec())),
        );
        backend.command_output(
            "nvidia-smi",
            &[
                "--query-gpu=index,name,uuid,memory.total,driver_version",
                "--format=csv,noheader,nounits",
            ],
            Ok(successful(
                b"0, NVIDIA H100 80GB HBM3, GPU-abc, 81920, 570.86.15\n".to_vec(),
            )),
        );
        backend.command_output(
            "nvidia-smi",
            &[],
            Ok(successful(
                b"| NVIDIA-SMI 570.86.15 Driver Version: 570.86.15 CUDA Version: 12.8 |\n".to_vec(),
            )),
        );
        backend.text(
            "/proc/cpuinfo",
            "processor : 0\nmodel name : Example CPU\nprocessor : 1\n",
        );
        backend.text("/proc/meminfo", "MemTotal:       67108864 kB\n");
        backend.text(
            "/etc/os-release",
            "NAME=\"Example Linux\"\nPRETTY_NAME=\"Example Linux 1\"\n",
        );
        backend
    }

    #[test]
    fn host_environment_snapshot_is_serializable_and_only_probes_the_host() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(Arc::clone(&events), &data_path);
        let mut progress = {
            let events = Arc::clone(&events);
            move |event: DoctorProgressEvent| {
                events
                    .lock()
                    .unwrap()
                    .push(format!("progress:{}", event.message));
            }
        };

        let snapshot = capture_host_environment(&mut backend, &mut progress);

        assert_eq!(snapshot.snapshot_version, HOST_ENVIRONMENT_VERSION);
        assert!(snapshot.captured_at_unix_ms.is_some());
        assert_eq!(snapshot.system.cpu_model.as_deref(), Some("Example CPU"));
        assert_eq!(snapshot.nvidia.gpus.len(), 1);
        assert!(snapshot.warnings.is_empty());

        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<HostEnvironment>(&json).unwrap(),
            snapshot
        );

        let events = events.lock().unwrap();
        let commands = events
            .iter()
            .filter_map(|event| event.strip_prefix("call:"))
            .collect::<Vec<_>>();
        assert_eq!(
            commands,
            [
                command_key("getconf", [OsString::from("GNU_LIBC_VERSION")]),
                command_key(
                    "nvidia-smi",
                    [
                        OsString::from("--query-gpu=index,name,uuid,memory.total,driver_version"),
                        OsString::from("--format=csv,noheader,nounits"),
                    ],
                ),
                command_key("nvidia-smi", []),
            ]
        );
        for (index, event) in events.iter().enumerate() {
            if event.starts_with("call:") {
                assert!(
                    index > 0 && events[index - 1].starts_with("progress:"),
                    "command did not have immediately preceding progress: {event}"
                );
            }
        }
    }

    #[test]
    fn host_environment_snapshot_tolerates_missing_nvidia() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);
        backend.commands.insert(
            command_key(
                "nvidia-smi",
                [
                    OsString::from("--query-gpu=index,name,uuid,memory.total,driver_version"),
                    OsString::from("--format=csv,noheader,nounits"),
                ],
            ),
            VecDeque::from([Err(anyhow!("command not found"))]),
        );

        let snapshot = capture_host_environment(&mut backend, &mut NoopDoctorProgress);

        assert_eq!(snapshot.system.cpu_model.as_deref(), Some("Example CPU"));
        assert!(!snapshot.nvidia.available);
        assert!(snapshot.nvidia.gpus.is_empty());
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|issue| issue.code == "nvidia-smi-unavailable"
                    && issue.severity == IssueSeverity::Warning)
        );
        assert!(
            snapshot
                .warnings
                .iter()
                .all(|issue| issue.severity == IssueSeverity::Warning)
        );
    }

    #[test]
    fn local_report_is_serializable_human_readable_and_ready() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(Arc::clone(&events), &data_path);
        let mut progress = {
            let events = Arc::clone(&events);
            move |event: DoctorProgressEvent| {
                events
                    .lock()
                    .unwrap()
                    .push(format!("progress:{}", event.message));
            }
        };
        let request = LocalDoctorRequest::new(fixture.path(), "data");

        let report = diagnose_local(&request, &mut backend, &mut progress);

        assert_eq!(report.status, DiagnosticStatus::Ready);
        assert!(report.repository.valid);
        assert_eq!(report.data_root.free_bytes, Some(900 * 1024));
        assert!(report.builds.iter().any(|build| build.usable));
        assert_eq!(
            report
                .tools
                .iter()
                .find(|tool| tool.name == "pixi-pack")
                .unwrap()
                .status,
            ToolStatus::AvailableViaPixiExec
        );
        assert_eq!(report.system.logical_cores, Some(2));
        assert_eq!(report.nvidia.cuda_version.as_deref(), Some("12.8"));
        assert!(report.to_string().contains("NVIDIA: 1 GPU(s)"));

        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<LocalDoctorReport>(&json).unwrap(),
            report
        );

        let events = events.lock().unwrap();
        for (index, event) in events.iter().enumerate() {
            if event.starts_with("call:") {
                assert!(
                    index > 0 && events[index - 1].starts_with("progress:"),
                    "command did not have immediately preceding progress: {event}"
                );
            }
        }
    }

    #[test]
    fn missing_optional_gpu_is_a_warning_not_a_failure() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);
        backend.commands.insert(
            command_key(
                "nvidia-smi",
                [
                    OsString::from("--query-gpu=index,name,uuid,memory.total,driver_version"),
                    OsString::from("--format=csv,noheader,nounits"),
                ],
            ),
            VecDeque::from([Err(anyhow!("command not found"))]),
        );

        let report = diagnose_local(
            &LocalDoctorRequest::new(fixture.path(), "data"),
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Warning);
        assert!(!report.nvidia.available);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "nvidia-smi-unavailable"
                    && issue.severity == IssueSeverity::Warning)
        );
    }

    #[test]
    fn missing_gpu_blocks_a_requested_sirius_workload() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);
        backend.commands.insert(
            command_key(
                "nvidia-smi",
                [
                    OsString::from("--query-gpu=index,name,uuid,memory.total,driver_version"),
                    OsString::from("--format=csv,noheader,nounits"),
                ],
            ),
            VecDeque::from([Err(anyhow!("command not found"))]),
        );

        let report = diagnose_local_for_workload(
            &LocalDoctorRequest::new(fixture.path(), "data"),
            true,
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        assert!(report.cuda_required);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "nvidia-required"
                    && issue.severity == IssueSeverity::Error)
        );
    }

    #[test]
    fn sirius_workload_requires_initialized_source_submodules() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);

        let report = diagnose_local_for_workload(
            &LocalDoctorRequest::new(fixture.path(), "data"),
            true,
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        assert_eq!(
            report
                .issues
                .iter()
                .filter(|issue| issue.code == "submodule-uninitialized")
                .count(),
            3
        );
    }

    #[test]
    fn missing_required_tool_blocks_with_an_action() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);
        backend.commands.insert(
            command_key("git", [OsString::from("--version")]),
            VecDeque::from([Ok(failed(127, "not found"))]),
        );

        let report = diagnose_local(
            &LocalDoctorRequest::new(fixture.path(), "data"),
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        let issue = report
            .issues
            .iter()
            .find(|issue| issue.code == "tool-git-missing")
            .unwrap();
        assert_eq!(issue.severity, IssueSeverity::Error);
        assert!(issue.action.contains("Install Git"));
    }

    #[test]
    fn worker_python_probe_uses_the_exact_selected_runtime_and_imports_native_duckdb() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(Arc::clone(&events), &data_path);
        backend.command_output(
            "/opt/benchmark/python",
            &["-c", PYTHON_RUNTIME_PROBE],
            Ok(successful("Python 3.12; DuckDB 1.4.0\n")),
        );
        let mut issues = Vec::new();

        let tools = inspect_tools_with_python(
            "/opt/benchmark/python",
            fixture.path(),
            &mut backend,
            &mut NoopDoctorProgress,
            &mut issues,
        );

        let python = tools.iter().find(|tool| tool.name == "python").unwrap();
        assert_eq!(python.executable.as_deref(), Some("/opt/benchmark/python"));
        assert_eq!(python.version.as_deref(), Some("Python 3.12; DuckDB 1.4.0"));
        let expected_call = format!(
            "call:{}",
            command_key(
                "/opt/benchmark/python",
                [OsString::from("-c"), OsString::from(PYTHON_RUNTIME_PROBE)]
            )
        );
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .any(|event| event == &expected_call)
        );
        assert!(issues.is_empty());
    }

    #[test]
    fn worker_python_import_failure_blocks_without_trying_another_interpreter() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(Arc::clone(&events), &data_path);
        backend.command_output(
            "/opt/benchmark/python",
            &["-c", PYTHON_RUNTIME_PROBE],
            Ok(failed(1, "ModuleNotFoundError: No module named '_duckdb'")),
        );
        let mut issues = Vec::new();

        let tools = inspect_tools_with_python(
            "/opt/benchmark/python",
            fixture.path(),
            &mut backend,
            &mut NoopDoctorProgress,
            &mut issues,
        );

        assert_eq!(
            tools
                .iter()
                .find(|tool| tool.name == "python")
                .unwrap()
                .status,
            ToolStatus::Missing
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "python-worker-runtime-unavailable"
                    && issue.severity == IssueSeverity::Error)
        );
        assert!(
            !events.lock().unwrap().iter().any(
                |event| event.starts_with("call:python3") || event.starts_with("call:python\0")
            )
        );
    }

    #[test]
    fn missing_ssh_warns_but_does_not_block_local_diagnostics() {
        let fixture = fixture();
        let events = Arc::new(Mutex::new(Vec::new()));
        let data_path = fixture.path().join("data");
        let mut backend = ready_backend(events, &data_path);
        backend.commands.insert(
            command_key("ssh", [OsString::from("-V")]),
            VecDeque::from([Ok(failed(127, "not found"))]),
        );

        let report = diagnose_local(
            &LocalDoctorRequest::new(fixture.path(), "data"),
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Warning);
        let ssh = report.tools.iter().find(|tool| tool.name == "ssh").unwrap();
        assert!(!ssh.required);
        assert_eq!(ssh.status, ToolStatus::Missing);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "tool-ssh-missing"
                    && issue.severity == IssueSeverity::Warning)
        );
    }

    #[test]
    fn strong_repo_check_rejects_a_nested_pixi_project() {
        let fixture = fixture();
        let nested = fixture.path().join("nested");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("pixi.toml"), b"[workspace]").unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut backend = FakeBackend::new(events);

        let report = diagnose_local(
            &LocalDoctorRequest::new(&nested, "data"),
            &mut backend,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        assert!(!report.repository.valid);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "repo-root-invalid")
        );
    }

    #[test]
    fn remote_doctor_reuses_read_only_compatibility_probe() {
        let mut runner = RemoteRunner {
            outputs: VecDeque::from([Ok(successful(
                b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 999.0\n570.86.15\n12.9\n8.0\nyes\n"
                    .to_vec(),
            ))]),
            command: None,
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut progress = {
            let events = Arc::clone(&events);
            move |event: DoctorProgressEvent| {
                events
                    .lock()
                    .unwrap()
                    .push(format!("progress:{}", event.message));
            }
        };
        let target = SshTarget::new("developer@example.test").unwrap();

        let report = diagnose_remote(&target, &mut runner, &mut progress);

        assert_eq!(report.status, DiagnosticStatus::Ready);
        assert_eq!(
            report
                .compatibility
                .as_ref()
                .unwrap()
                .nvidia_driver_version
                .as_deref(),
            Some("570.86.15")
        );
        assert_eq!(
            report
                .compatibility
                .as_ref()
                .unwrap()
                .nvidia_cuda_version
                .as_deref(),
            Some("12.9")
        );
        assert!(report.execution_target.is_some());
        let command = runner.command.unwrap();
        assert_eq!(command.program, OsString::from("ssh"));
        assert_eq!(command.stdin, CommandInput::Null);
        assert!(
            command
                .args
                .iter()
                .any(|argument| argument == "BatchMode=yes")
        );
        assert_eq!(events.lock().unwrap().len(), 2);
    }

    #[test]
    fn remote_probe_failure_is_an_actionable_report() {
        let mut runner = RemoteRunner {
            outputs: VecDeque::from([Err(anyhow!("SSH connection refused"))]),
            command: None,
        };
        let target = SshTarget::new("developer@example.test").unwrap();

        let report = diagnose_remote(&target, &mut runner, &mut NoopDoctorProgress);

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        assert!(report.compatibility.is_none());
        assert_eq!(report.issues[0].code, "remote-probe-failed");
        assert!(report.issues[0].action.contains("BatchMode=yes"));
    }

    #[test]
    fn remote_doctor_validates_the_requested_repository_after_host_probe() {
        let local = fixture();
        let local_state = local_repository_state(local.path()).unwrap();
        let mut runner = RemoteRunner {
            outputs: VecDeque::from([
                Ok(successful("pixi 0.71.0\n")),
                Ok(successful(
                    b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 999.0\n570.86.15\n12.9\n8.0\nyes\n"
                        .to_vec(),
                )),
                Ok(successful(format!(
                    "ready\n{}\n{}\n{}\nclean\n",
                    local_state.manifest_sha256, local_state.lock_sha256, local_state.git_commit,
                ))),
            ]),
            command: None,
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut progress = {
            let events = Arc::clone(&events);
            move |event: DoctorProgressEvent| {
                events.lock().unwrap().push(event.message);
            }
        };
        let target = SshTarget::new("developer@example.test").unwrap();

        let report = diagnose_remote_with_repository(
            &target,
            local.path(),
            Path::new("/srv/sirius"),
            &mut runner,
            &mut progress,
        );

        assert_eq!(report.status, DiagnosticStatus::Ready);
        let repository = report.repository.as_ref().unwrap();
        assert_eq!(repository.path, Path::new("/srv/sirius"));
        assert!(repository.checked);
        assert!(repository.valid);
        assert!(repository.state.is_some());
        assert_eq!(events.lock().unwrap().len(), 5);
        assert!(report.to_string().contains("/srv/sirius (valid)"));
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<RemoteDoctorReport>(&json).unwrap(),
            report
        );
    }

    #[test]
    fn invalid_remote_repository_blocks_with_an_action() {
        let local = fixture();
        let mut runner = RemoteRunner {
            outputs: VecDeque::from([
                Ok(successful("pixi 0.71.0\n")),
                Ok(successful(
                    b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 999.0\n570.86.15\n12.9\n8.0\nyes\n"
                        .to_vec(),
                )),
                Ok(failed(3, "remote checkout is missing pixi.lock")),
            ]),
            command: None,
        };
        let target = SshTarget::new("developer@example.test").unwrap();

        let report = diagnose_remote_with_repository(
            &target,
            local.path(),
            Path::new("/srv/sirius"),
            &mut runner,
            &mut NoopDoctorProgress,
        );

        assert_eq!(report.status, DiagnosticStatus::Blocked);
        assert!(report.repository.as_ref().unwrap().checked);
        assert!(!report.repository.as_ref().unwrap().valid);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "remote-repository-invalid"
                    && issue.message.contains("pixi.lock")
                    && issue.action.contains("--remote-repo"))
        );
    }

    #[test]
    fn parsers_cover_host_and_gpu_formats() {
        assert_eq!(
            parse_df_free_bytes(
                "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/x 100 1 42 1% /x\n"
            )
            .unwrap(),
            42 * 1024
        );
        assert_eq!(
            parse_cpu_model("processor: 0\nmodel name : AMD EPYC\n").as_deref(),
            Some("AMD EPYC")
        );
        assert_eq!(parse_cpu_cores("processor: 0\nprocessor: 1\n"), Some(2));
        assert_eq!(parse_ram_bytes("MemTotal: 1024 kB\n"), Some(1024 * 1024));
        assert_eq!(
            parse_os_release("NAME=Linux\nPRETTY_NAME=\"Useful Linux\"\n").as_deref(),
            Some("Useful Linux")
        );
        assert_eq!(
            parse_cuda_version("Driver Version: 570 CUDA Version: 12.8 |").as_deref(),
            Some("12.8")
        );
        let (gpus, driver, malformed) =
            parse_nvidia_query("0, NVIDIA A100, GPU-one, 40960, 570.1\nbad\n");
        assert_eq!(gpus.len(), 1);
        assert_eq!(driver.as_deref(), Some("570.1"));
        assert_eq!(malformed, 1);
    }

    #[derive(Debug)]
    struct RemoteRunner {
        outputs: VecDeque<anyhow::Result<ProcessOutput>>,
        command: Option<CommandSpec>,
    }

    impl CommandRunner for RemoteRunner {
        fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
            self.command = Some(command.clone());
            self.outputs.pop_front().unwrap()
        }
    }

    fn command_key(program: &str, arguments: impl IntoIterator<Item = OsString>) -> String {
        std::iter::once(program.to_owned())
            .chain(
                arguments
                    .into_iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            )
            .collect::<Vec<_>>()
            .join("\0")
    }
}
