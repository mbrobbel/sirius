//! Local Pixi-Pack caching and an SSH-only remote execution transport.
//!
//! The transport has no scheduler or resident service. It stages a self-extracting
//! environment and a runner binary, sends versioned JSON protocols on stdin, and
//! downloads a checksummed result archive.

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};

use crate::{cancel, progress::Reporter};

pub const REMOTE_PROTOCOL_VERSION: u32 = 1;
pub const REMOTE_RESULT_VERSION: u32 = 1;
pub const PACK_CACHE_KEY_VERSION: u32 = 2;
const PACK_RECEIPT_VERSION: u32 = 2;
const PROBE_MARKER: &str = "sirius-runner-probe-v3";
const PROBE_SCRIPT: &str = r#"set -eu
printf '%s\n' 'sirius-runner-probe-v3'
uname -s
uname -m
glibc=$( (getconf GNU_LIBC_VERSION 2>/dev/null || true) | head -n 1)
if command -v timeout >/dev/null 2>&1; then
  nvidia=$(timeout 10 nvidia-smi 2>/dev/null || true)
  compute=$(timeout 10 nvidia-smi --query-gpu=index,compute_cap --format=csv,noheader,nounits 2>/dev/null | sort -t, -k1,1n | cut -d, -f2 | tr '\n' ',' | sed 's/,$//' || true)
else
  nvidia=
  compute=
fi
driver=$(printf '%s\n' "$nvidia" | sed -n 's/.*Driver Version: \([^ |]*\).*/\1/p' | head -n 1)
cuda=$(printf '%s\n' "$nvidia" | sed -n 's/.*CUDA Version: \([^ |]*\).*/\1/p' | head -n 1)
if command -v flock >/dev/null 2>&1; then flock_available=yes; else flock_available=no; fi
printf '%s\n' "$glibc" "$driver" "$cuda" "$compute" "$flock_available"
"#;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)
            .with_context(|| format!("hashing {}", path.display()))?;
        Ok(Self(hasher.finalize().into()))
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        ensure!(
            value.len() == 64,
            "SHA-256 digest must contain exactly 64 hexadecimal characters"
        );

        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            bytes[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }

    pub fn as_hex(self) -> String {
        let mut value = String::with_capacity(64);
        for byte in self.0 {
            use fmt::Write as _;
            write!(&mut value, "{byte:02x}").expect("writing to a String cannot fail");
        }
        value
    }
}

fn hex_nibble(byte: u8) -> anyhow::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("SHA-256 digest contains a non-hexadecimal character"),
    }
}

impl fmt::Debug for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sha256Digest")
            .field(&self.as_hex())
            .finish()
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PackCacheKey(Sha256Digest);

impl PackCacheKey {
    pub fn as_str(&self) -> String {
        self.0.as_hex()
    }
}

impl fmt::Display for PackCacheKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PixiPackFormat {
    SelfExtractingShellV1,
}

impl PixiPackFormat {
    fn cache_value(self) -> &'static str {
        match self {
            Self::SelfExtractingShellV1 => "self-extracting-shell-v1",
        }
    }

    fn output_name(self) -> &'static str {
        match self {
            Self::SelfExtractingShellV1 => "environment.sh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackKeyInputs {
    pub manifest_digest: Sha256Digest,
    pub lock_digest: Sha256Digest,
    pub environment: String,
    pub target_platform: String,
    pub cuda_profile: String,
    pub pixi_pack_version: String,
    pub format: PixiPackFormat,
}

impl PackKeyInputs {
    pub fn cache_key(&self) -> PackCacheKey {
        let mut hasher = Sha256::new();
        hasher.update(b"sirius-runner-pixi-pack-cache-key");
        hash_field(&mut hasher, &PACK_CACHE_KEY_VERSION.to_be_bytes());
        hash_field(&mut hasher, &self.manifest_digest.0);
        hash_field(&mut hasher, &self.lock_digest.0);
        hash_field(&mut hasher, self.environment.as_bytes());
        hash_field(&mut hasher, self.target_platform.as_bytes());
        hash_field(&mut hasher, self.cuda_profile.as_bytes());
        hash_field(&mut hasher, self.pixi_pack_version.as_bytes());
        hash_field(&mut hasher, self.format.cache_value().as_bytes());
        PackCacheKey(Sha256Digest(hasher.finalize().into()))
    }
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixiPackRequest {
    pub manifest_path: PathBuf,
    pub lock_path: PathBuf,
    pub environment: String,
    pub target_platform: String,
    pub cuda_profile: String,
    pub cuda_required: bool,
    pub pixi_pack_version: String,
    pub format: PixiPackFormat,
}

impl PixiPackRequest {
    pub fn key_inputs(&self) -> anyhow::Result<PackKeyInputs> {
        for (name, value) in [
            ("environment", self.environment.as_str()),
            ("CUDA profile", self.cuda_profile.as_str()),
            ("pixi-pack version", self.pixi_pack_version.as_str()),
        ] {
            ensure!(
                !value.trim().is_empty() && !value.chars().any(char::is_control),
                "Pixi pack {name} cannot be empty or contain control characters"
            );
        }
        ensure!(
            matches!(self.target_platform.as_str(), "linux-64" | "linux-aarch64"),
            "unsupported Pixi Pack platform `{}`; expected linux-64 or linux-aarch64",
            self.target_platform
        );
        Ok(PackKeyInputs {
            manifest_digest: Sha256Digest::from_file(&self.manifest_path)?,
            lock_digest: Sha256Digest::from_file(&self.lock_path)?,
            environment: self.environment.clone(),
            target_platform: self.target_platform.clone(),
            cuda_profile: self.cuda_profile.clone(),
            pixi_pack_version: self.pixi_pack_version.clone(),
            format: self.format,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandInput {
    Null,
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutputTarget {
    Capture,
    File(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    pub stdin: CommandInput,
    pub stdout: CommandOutputTarget,
}

impl CommandSpec {
    fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            stdin: CommandInput::Null,
            stdout: CommandOutputTarget::Capture,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ProcessOutput {
    #[cfg(test)]
    fn success(stdout: impl Into<Vec<u8>>) -> Self {
        Self {
            success: true,
            code: Some(0),
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }
}

pub trait CommandRunner {
    fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput>;

    fn run_with_live_stderr(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
        self.run(command)
    }
}

#[derive(Debug, Default)]
pub struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
        run_system_command(command, false)
    }

    fn run_with_live_stderr(&mut self, command: &CommandSpec) -> anyhow::Result<ProcessOutput> {
        run_system_command(command, true)
    }
}

fn run_system_command(command: &CommandSpec, live_stderr: bool) -> anyhow::Result<ProcessOutput> {
    let mut process = Command::new(&command.program);
    process.args(&command.args).stderr(if live_stderr {
        Stdio::inherit()
    } else {
        Stdio::piped()
    });
    if let Some(current_dir) = &command.current_dir {
        process.current_dir(current_dir);
    }

    match &command.stdin {
        CommandInput::Null => {
            process.stdin(Stdio::null());
        }
        CommandInput::Bytes(_) => {
            process.stdin(Stdio::piped());
        }
        CommandInput::File(path) => {
            let file = File::open(path)
                .with_context(|| format!("opening command input {}", path.display()))?;
            process.stdin(Stdio::from(file));
        }
    }

    match &command.stdout {
        CommandOutputTarget::Capture => {
            process.stdout(Stdio::piped());
        }
        CommandOutputTarget::File(path) => {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .with_context(|| format!("creating command output {}", path.display()))?;
            process.stdout(Stdio::from(file));
        }
    }

    crate::process::configure_process_group(&mut process);
    let mut child = process
        .spawn()
        .with_context(|| format!("starting {:?}", command.program))?;
    if let CommandInput::Bytes(input) = &command.stdin
        && let Err(error) = child
            .stdin
            .take()
            .context("child process did not expose stdin")?
            .write_all(input)
            .context("writing command stdin")
    {
        crate::process::terminate_child_process_group(&mut child);
        return Err(error);
    }
    let stdout_reader = child.stdout.take().map(spawn_pipe_reader);
    let stderr_reader = child.stderr.take().map(spawn_pipe_reader);
    let started = Instant::now();
    let mut last_heartbeat = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                crate::process::terminate_child_process_group(&mut child);
                return Err(error).context("waiting for command");
            }
        }
        if let Err(error) = cancel::check() {
            crate::process::terminate_child_process_group(&mut child);
            return Err(error).context(format!("cancelling {:?}", command.program));
        }
        if last_heartbeat.elapsed() >= Duration::from_secs(15) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "Still waiting for {:?} ({:.0}s)",
                command.program,
                started.elapsed().as_secs_f64()
            );
            last_heartbeat = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let stdout = join_pipe_reader(stdout_reader)?.unwrap_or_default();
    let stderr = join_pipe_reader(stderr_reader)?.unwrap_or_default();
    Ok(ProcessOutput {
        success: status.success(),
        code: status.code(),
        stdout,
        stderr: if live_stderr { Vec::new() } else { stderr },
    })
}

type PipeReader = std::thread::JoinHandle<std::io::Result<Vec<u8>>>;

fn spawn_pipe_reader(mut pipe: impl Read + Send + 'static) -> PipeReader {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

fn join_pipe_reader(reader: Option<PipeReader>) -> anyhow::Result<Option<Vec<u8>>> {
    reader
        .map(|reader| {
            reader
                .join()
                .map_err(|_| anyhow::anyhow!("command output reader panicked"))?
                .context("reading command output")
        })
        .transpose()
}

fn run_checked(
    runner: &mut impl CommandRunner,
    command: &CommandSpec,
    context: &str,
) -> anyhow::Result<ProcessOutput> {
    let output = runner.run(command).with_context(|| context.to_owned())?;
    if !output.success {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{context} failed with exit code {}: {}",
            output
                .code
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
            stderr.trim()
        );
    }
    Ok(output)
}

fn run_checked_live(
    runner: &mut impl CommandRunner,
    command: &CommandSpec,
    context: &str,
) -> anyhow::Result<ProcessOutput> {
    let output = runner
        .run_with_live_stderr(command)
        .with_context(|| context.to_owned())?;
    if !output.success {
        bail!(
            "{context} failed with exit code {}",
            output
                .code
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
        );
    }
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteStage {
    ProbeCompatibility,
    ProbeRepository,
    ValidateInvocation,
    CheckLocalTool,
    CheckLocalPack,
    BuildLocalPack,
    CheckRemotePack,
    UploadPack,
    UnpackPack,
    PrepareJob,
    UploadRunner,
    UploadInput,
    Handshake,
    Execute,
    DownloadResult,
    CleanupJob,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
    pub stage: RemoteStage,
    pub message: String,
}

pub trait ProgressSink {
    fn emit(&mut self, event: ProgressEvent);
}

impl<F> ProgressSink for F
where
    F: FnMut(ProgressEvent),
{
    fn emit(&mut self, event: ProgressEvent) {
        self(event);
    }
}

struct PackRunLockProgress<'a, S> {
    sink: &'a mut S,
}

impl<S: ProgressSink> Reporter for PackRunLockProgress<'_, S> {
    fn status(&mut self, message: &str) -> std::io::Result<()> {
        progress(self.sink, RemoteStage::BuildLocalPack, message.to_owned());
        Ok(())
    }

    fn detail(&mut self, message: &str) -> std::io::Result<()> {
        self.status(message)
    }
}

#[derive(Debug, Default)]
pub struct NoopProgress;

impl ProgressSink for NoopProgress {
    fn emit(&mut self, _event: ProgressEvent) {}
}

fn progress(sink: &mut impl ProgressSink, stage: RemoteStage, message: impl Into<String>) {
    sink.emit(ProgressEvent {
        stage,
        message: message.into(),
    });
}

fn sha256_file_with_progress(
    path: &Path,
    label: &str,
    stage: RemoteStage,
    progress_sink: &mut impl ProgressSink,
) -> anyhow::Result<Sha256Digest> {
    progress(
        progress_sink,
        stage,
        format!("checksumming {label}: {}", path.display()),
    );
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let started = Instant::now();
    let mut last_heartbeat = started;
    let mut bytes = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        cancel::check()?;
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("reading {}", path.display()))?;
        if count == 0 {
            break;
        }
        bytes = bytes.saturating_add(count as u64);
        hasher.update(&buffer[..count]);
        if last_heartbeat.elapsed() >= Duration::from_secs(10) {
            progress(
                progress_sink,
                stage,
                format!(
                    "still checksumming {label}: {} read ({:.0}s)",
                    human_bytes(bytes),
                    started.elapsed().as_secs_f64()
                ),
            );
            last_heartbeat = Instant::now();
        }
    }
    progress(
        progress_sink,
        stage,
        format!(
            "checksummed {label}: {} ({:.1}s)",
            human_bytes(bytes),
            started.elapsed().as_secs_f64()
        ),
    );
    Ok(Sha256Digest(hasher.finalize().into()))
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackReceipt {
    pub schema_version: u32,
    pub key: PackCacheKey,
    pub inputs: PackKeyInputs,
    pub archive_sha256: Sha256Digest,
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedEnvironment {
    pub key: PackCacheKey,
    pub archive_path: PathBuf,
    pub archive_sha256: Sha256Digest,
    pub archive_bytes: u64,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixiPackCache {
    root: PathBuf,
}

impl PixiPackCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Checks an existing cache entry without creating or changing anything.
    pub fn lookup(
        &self,
        request: &PixiPackRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<Option<PackedEnvironment>> {
        ensure!(
            request.manifest_path.is_file(),
            "Pixi manifest does not exist: {}",
            request.manifest_path.display()
        );
        ensure!(
            request.lock_path.is_file(),
            "Pixi lock file does not exist: {}",
            request.lock_path.display()
        );
        let inputs = request.key_inputs()?;
        self.load(&inputs.cache_key(), &inputs, progress_sink)
    }

    pub fn ensure(
        &self,
        runner: &mut impl CommandRunner,
        request: &PixiPackRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<PackedEnvironment> {
        progress(
            progress_sink,
            RemoteStage::CheckLocalPack,
            "checking local Pixi pack cache",
        );
        ensure!(
            request.manifest_path.is_file(),
            "Pixi manifest does not exist: {}",
            request.manifest_path.display()
        );
        ensure!(
            request.lock_path.is_file(),
            "Pixi lock file does not exist: {}",
            request.lock_path.display()
        );
        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating pack cache {}", self.root.display()))?;

        let inputs = request.key_inputs()?;
        let key = inputs.cache_key();
        let run_lock = {
            let mut lock_progress = PackRunLockProgress {
                sink: progress_sink,
            };
            crate::run_lock::RunLock::acquire(&mut lock_progress)?
        };
        match self.load(&key, &inputs, progress_sink) {
            Ok(Some(cached)) => {
                progress(
                    progress_sink,
                    RemoteStage::CheckLocalPack,
                    format!("using cached Pixi pack {key}"),
                );
                drop(run_lock);
                return Ok(cached);
            }
            Ok(None) => {}
            Err(error) => progress(
                progress_sink,
                RemoteStage::CheckLocalPack,
                format!("cached Pixi pack {key} is invalid and will be rebuilt: {error:#}"),
            ),
        }

        let lock_path = self.root.join(format!(".{key}.lock"));
        let lock = CacheLock::acquire(&lock_path, progress_sink)?;
        match self.load(&key, &inputs, progress_sink) {
            Ok(Some(cached)) => {
                progress(
                    progress_sink,
                    RemoteStage::CheckLocalPack,
                    format!("using cached Pixi pack {key}"),
                );
                return Ok(cached);
            }
            Ok(None) => {}
            Err(error) => self.discard_invalid(&key, &error, progress_sink)?,
        }

        progress(
            progress_sink,
            RemoteStage::BuildLocalPack,
            format!(
                "building Pixi pack for {} ({})",
                request.target_platform, request.cuda_profile
            ),
        );
        let temp_path = self.root.join(format!(
            ".{key}.tmp-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temp_path)
            .with_context(|| format!("creating temporary pack {}", temp_path.display()))?;
        let mut cleanup = RemoveOnDrop::directory(temp_path.clone());

        let manifest_path = fs::canonicalize(&request.manifest_path)
            .with_context(|| format!("resolving {}", request.manifest_path.display()))?;
        let package_cache = self.root.join("packages");
        fs::create_dir_all(&package_cache)
            .with_context(|| format!("creating package cache {}", package_cache.display()))?;

        let mut command = CommandSpec::new("pixi");
        command.current_dir = Some(temp_path.clone());
        command.args = vec![
            "exec".into(),
            "--spec".into(),
            format!("pixi-pack={}", request.pixi_pack_version).into(),
            "--".into(),
            "pixi-pack".into(),
            "--environment".into(),
            request.environment.clone().into(),
            "--platform".into(),
            request.target_platform.clone().into(),
            "--create-executable".into(),
            "--use-cache".into(),
            package_cache.into_os_string(),
            manifest_path.into_os_string(),
        ];
        run_checked_live(runner, &command, "building Pixi pack")?;

        let current_inputs = request.key_inputs()?;
        ensure!(
            current_inputs.manifest_digest == inputs.manifest_digest
                && current_inputs.lock_digest == inputs.lock_digest,
            "pixi-pack inputs changed while the environment was being built; refusing to cache it"
        );

        let archive_path = temp_path.join(request.format.output_name());
        ensure!(
            archive_path.is_file(),
            "pixi-pack did not create {}",
            archive_path.display()
        );
        let archive_sha256 = sha256_file_with_progress(
            &archive_path,
            "new Pixi pack",
            RemoteStage::BuildLocalPack,
            progress_sink,
        )?;
        let archive_bytes = archive_path
            .metadata()
            .with_context(|| format!("reading metadata for {}", archive_path.display()))?
            .len();
        let receipt = PackReceipt {
            schema_version: PACK_RECEIPT_VERSION,
            key: key.clone(),
            inputs,
            archive_sha256,
            archive_bytes,
        };
        fs::write(
            temp_path.join("receipt.json"),
            serde_json::to_vec_pretty(&receipt)?,
        )
        .with_context(|| format!("writing receipt for pack {key}"))?;

        let final_path = self.root.join(key.as_str());
        fs::rename(&temp_path, &final_path).with_context(|| {
            format!(
                "publishing Pixi pack {} to {}",
                temp_path.display(),
                final_path.display()
            )
        })?;
        cleanup.disarm();
        drop(lock);
        drop(run_lock);

        Ok(PackedEnvironment {
            key,
            archive_path: final_path.join(request.format.output_name()),
            archive_sha256,
            archive_bytes,
            cache_hit: false,
        })
    }

    fn load(
        &self,
        key: &PackCacheKey,
        inputs: &PackKeyInputs,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<Option<PackedEnvironment>> {
        let directory = self.root.join(key.as_str());
        if !directory.exists() {
            return Ok(None);
        }

        let receipt_path = directory.join("receipt.json");
        ensure!(
            receipt_path.is_file(),
            "incomplete Pixi pack cache entry {}; remove it and retry",
            directory.display()
        );
        let receipt: PackReceipt = serde_json::from_slice(
            &fs::read(&receipt_path)
                .with_context(|| format!("reading {}", receipt_path.display()))?,
        )
        .with_context(|| format!("parsing {}", receipt_path.display()))?;
        ensure!(
            receipt.schema_version == PACK_RECEIPT_VERSION,
            "unsupported Pixi pack receipt version {} in {}",
            receipt.schema_version,
            receipt_path.display()
        );
        ensure!(
            &receipt.key == key && &receipt.inputs == inputs,
            "Pixi pack cache receipt does not match its cache key in {}",
            receipt_path.display()
        );

        let archive_path = directory.join(inputs.format.output_name());
        let metadata = archive_path
            .metadata()
            .with_context(|| format!("reading cached pack {}", archive_path.display()))?;
        ensure!(
            metadata.is_file() && metadata.len() == receipt.archive_bytes,
            "cached Pixi pack has changed: {}",
            archive_path.display()
        );
        let archive_sha256 = sha256_file_with_progress(
            &archive_path,
            "cached Pixi pack",
            RemoteStage::CheckLocalPack,
            progress_sink,
        )?;
        ensure!(
            archive_sha256 == receipt.archive_sha256,
            "cached Pixi pack checksum does not match its receipt: {}",
            archive_path.display()
        );
        Ok(Some(PackedEnvironment {
            key: key.clone(),
            archive_path,
            archive_sha256: receipt.archive_sha256,
            archive_bytes: receipt.archive_bytes,
            cache_hit: true,
        }))
    }

    fn discard_invalid(
        &self,
        key: &PackCacheKey,
        error: &anyhow::Error,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<()> {
        let directory = self.root.join(key.as_str());
        if !directory.exists() {
            return Ok(());
        }
        let quarantine = self.root.join(format!(
            ".invalid-{key}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        progress(
            progress_sink,
            RemoteStage::CheckLocalPack,
            format!("discarding invalid cached Pixi pack {key}: {error:#}"),
        );
        fs::rename(&directory, &quarantine).with_context(|| {
            format!(
                "quarantining invalid Pixi pack {} as {}",
                directory.display(),
                quarantine.display()
            )
        })?;
        fs::remove_dir_all(&quarantine)
            .with_context(|| format!("removing quarantined Pixi pack {}", quarantine.display()))
    }
}

struct CacheLock {
    file: File,
}

impl CacheLock {
    fn acquire(path: &Path, progress_sink: &mut impl ProgressSink) -> anyhow::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening Pixi pack lock {}", path.display()))?;
        let started = Instant::now();
        let mut last_update = Instant::now();
        loop {
            cancel::check()?;
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if last_update.elapsed() >= Duration::from_secs(15) {
                        progress(
                            progress_sink,
                            RemoteStage::CheckLocalPack,
                            format!(
                                "waiting for another process to build the Pixi pack ({:.0}s)",
                                started.elapsed().as_secs_f64()
                            ),
                        );
                        last_update = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("locking Pixi pack cache {}", path.display()));
                }
            }
        }
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

enum RemovalKind {
    Directory,
    File,
}

struct RemoveOnDrop {
    path: PathBuf,
    kind: RemovalKind,
    armed: bool,
}

impl RemoveOnDrop {
    fn directory(path: PathBuf) -> Self {
        Self {
            path,
            kind: RemovalKind::Directory,
            armed: true,
        }
    }

    fn file(path: PathBuf) -> Self {
        Self {
            path,
            kind: RemovalKind::File,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.kind {
            RemovalKind::Directory => {
                let _ = fs::remove_dir_all(&self.path);
            }
            RemovalKind::File => {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SshTarget(String);

impl SshTarget {
    pub fn new(target: impl Into<String>) -> anyhow::Result<Self> {
        let target = target.into();
        ensure!(!target.is_empty(), "SSH target cannot be empty");
        ensure!(
            !target.starts_with('-'),
            "SSH target cannot begin with a hyphen"
        );
        ensure!(
            target.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'@' | b'.' | b'_' | b'-' | b':' | b'[' | b']' | b'%' | b'+'
                    )
            }),
            "SSH target contains unsupported characters"
        );
        Ok(Self(target))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RemoteRunId(String);

impl RemoteRunId {
    pub fn new(id: impl Into<String>) -> anyhow::Result<Self> {
        let id = id.into();
        ensure!(
            !id.is_empty() && id.len() <= 128,
            "remote run ID must contain between 1 and 128 characters"
        );
        ensure!(id != "." && id != "..", "invalid remote run ID");
        ensure!(
            id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
            "remote run ID may contain only ASCII letters, digits, '.', '_' and '-'"
        );
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RemoteRunId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCompatibility {
    pub os: String,
    pub architecture: String,
    pub glibc_version: Option<String>,
    pub nvidia_driver_version: Option<String>,
    pub nvidia_cuda_version: Option<String>,
    pub nvidia_compute_capabilities: Vec<String>,
    pub flock_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteRepositoryState {
    pub manifest_sha256: Sha256Digest,
    pub lock_sha256: Sha256Digest,
    pub git_commit: String,
    pub git_dirty: bool,
}

impl RemoteCompatibility {
    pub fn ensure_compatible(
        &self,
        target_platform: &str,
        cuda_profile: &str,
    ) -> anyhow::Result<()> {
        ensure!(
            self.os.eq_ignore_ascii_case("linux"),
            "remote OS `{}` is not supported; SSH execution currently requires Linux",
            self.os
        );
        ensure!(
            self.flock_available,
            "remote host is missing `flock`, which is required to serialize benchmark work"
        );

        let architecture_matches = if target_platform.starts_with("linux-64") {
            matches!(self.architecture.as_str(), "x86_64" | "amd64")
        } else if target_platform.starts_with("linux-aarch64") {
            matches!(self.architecture.as_str(), "aarch64" | "arm64")
        } else {
            bail!("unsupported Pixi target platform `{target_platform}`");
        };
        ensure!(
            architecture_matches,
            "remote architecture `{}` is incompatible with Pixi target `{target_platform}`",
            self.architecture
        );
        ensure!(
            self.glibc_version.is_some(),
            "remote did not report a glibc version"
        );
        if !matches!(cuda_profile, "" | "none" | "cpu") {
            ensure!(
                self.nvidia_driver_version.is_some(),
                "remote did not report an NVIDIA driver required by CUDA profile `{cuda_profile}`"
            );
            let reported = self.nvidia_cuda_version.as_deref().with_context(|| {
                format!(
                    "remote did not report the driver CUDA compatibility required by CUDA profile `{cuda_profile}`"
                )
            })?;
            let required = match cuda_profile {
                "cuda12" => (12, 9),
                "cuda13" => (13, 2),
                _ => bail!("unsupported CUDA profile `{cuda_profile}`"),
            };
            let reported_version = parse_version(reported).with_context(|| {
                format!("remote reported an invalid CUDA compatibility version `{reported}`")
            })?;
            ensure!(
                reported_version >= required,
                "remote CUDA compatibility {reported} is too old for profile `{cuda_profile}` (requires {}.{})",
                required.0,
                required.1
            );
            ensure!(
                self.nvidia_compute_capabilities
                    .first()
                    .and_then(|capability| parse_version(capability).ok())
                    .is_some_and(|capability| supports_compute_capability(
                        cuda_profile,
                        capability
                    )),
                "remote default GPU has no compute capability supported by CUDA profile `{cuda_profile}`; reported devices in index order: {}",
                if self.nvidia_compute_capabilities.is_empty() {
                    "none".to_owned()
                } else {
                    self.nvidia_compute_capabilities.join(", ")
                }
            );
        }
        Ok(())
    }
}

fn supports_compute_capability(profile: &str, capability: (u32, u32)) -> bool {
    match profile {
        "cuda12" => matches!(capability, (7, 5) | (8, 0) | (8, 6) | (8, 9) | (9, 0)),
        "cuda13" => matches!(
            capability,
            (7, 5) | (8, 0) | (8, 6) | (8, 9) | (9, 0) | (10, 0) | (12, 0)
        ),
        _ => false,
    }
}

fn parse_version(version: &str) -> anyhow::Result<(u32, u32)> {
    let mut components = version.trim().split('.');
    let major = components
        .next()
        .context("version is empty")?
        .parse()
        .context("version does not begin with a number")?;
    let minor = components
        .next()
        .unwrap_or("0")
        .parse()
        .context("version minor component is not a number")?;
    Ok((major, minor))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub sha256: Sha256Digest,
    pub bytes: u64,
}

impl ArtifactDescriptor {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            sha256: Sha256Digest::from_file(path)?,
            bytes: path
                .metadata()
                .with_context(|| format!("reading metadata for {}", path.display()))?
                .len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeRequest {
    pub protocol_version: u32,
    pub client_version: String,
    pub plan_version: u32,
    pub pack_key: PackCacheKey,
    pub runner: ArtifactDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeResponse {
    pub protocol_version: u32,
    pub runner_version: String,
    pub plan_version: u32,
    pub pack_key: PackCacheKey,
    pub runner: ArtifactDescriptor,
}

impl HandshakeResponse {
    pub fn validate(&self, request: &HandshakeRequest) -> anyhow::Result<()> {
        ensure!(
            self.protocol_version == REMOTE_PROTOCOL_VERSION,
            "remote protocol version {} is incompatible with local version {}",
            self.protocol_version,
            REMOTE_PROTOCOL_VERSION
        );
        ensure!(
            self.plan_version == request.plan_version,
            "remote accepted plan version {}, expected {}",
            self.plan_version,
            request.plan_version
        );
        ensure!(
            self.pack_key == request.pack_key,
            "remote activated a different Pixi pack"
        );
        ensure!(
            self.runner == request.runner,
            "remote runner does not match the uploaded runner"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemotePlan<T> {
    pub protocol_version: u32,
    pub plan_version: u32,
    pub run_id: RemoteRunId,
    pub pack_key: PackCacheKey,
    pub input: Option<ArtifactDescriptor>,
    pub payload: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRunStatus {
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultEnvelope<T> {
    pub protocol_version: u32,
    pub result_version: u32,
    pub run_id: RemoteRunId,
    pub status: RemoteRunStatus,
    pub result: Option<T>,
    pub error: Option<RemoteFailure>,
}

impl<T> ResultEnvelope<T> {
    pub fn validate(&self, run_id: &RemoteRunId) -> anyhow::Result<()> {
        ensure!(
            self.protocol_version == REMOTE_PROTOCOL_VERSION,
            "result protocol version {} is incompatible with local version {}",
            self.protocol_version,
            REMOTE_PROTOCOL_VERSION
        );
        ensure!(
            self.result_version == REMOTE_RESULT_VERSION,
            "result schema version {} is incompatible with local version {}",
            self.result_version,
            REMOTE_RESULT_VERSION
        );
        ensure!(
            &self.run_id == run_id,
            "remote result has a different run ID"
        );
        match self.status {
            RemoteRunStatus::Completed => {
                ensure!(
                    self.result.is_some() && self.error.is_none(),
                    "completed result envelope must contain a result and no error"
                );
            }
            RemoteRunStatus::Failed => {
                ensure!(
                    self.error.is_some(),
                    "failed result envelope must contain an error"
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RemoteRunRequest<T> {
    pub target: SshTarget,
    pub client_version: String,
    pub pack: PixiPackRequest,
    pub runner_binary: PathBuf,
    pub input_archive: Option<PathBuf>,
    pub run_id: RemoteRunId,
    pub plan_version: u32,
    pub payload: T,
    pub result_archive: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RemoteRunOutcome {
    pub compatibility: RemoteCompatibility,
    pub pack_key: PackCacheKey,
    pub local_pack: Option<PackedEnvironment>,
    pub remote_pack_cache_hit: bool,
    pub handshake: HandshakeResponse,
    pub envelope: ResultEnvelope<ArtifactDescriptor>,
    pub local_result_archive: Option<PathBuf>,
}

pub struct RemoteExecutor<R> {
    command_runner: R,
    pack_cache: PixiPackCache,
}

impl<R> RemoteExecutor<R>
where
    R: CommandRunner,
{
    pub fn new(command_runner: R, pack_cache: PixiPackCache) -> Self {
        Self {
            command_runner,
            pack_cache,
        }
    }

    pub fn command_runner(&self) -> &R {
        &self.command_runner
    }

    pub fn probe(
        &mut self,
        target: &SshTarget,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteCompatibility> {
        progress(
            progress_sink,
            RemoteStage::ProbeCompatibility,
            format!("probing {}", target.as_str()),
        );
        probe_compatibility(&mut self.command_runner, target)
    }

    pub fn lookup_local_pack(
        &self,
        request: &PixiPackRequest,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<Option<PackedEnvironment>> {
        progress(
            progress_sink,
            RemoteStage::CheckLocalPack,
            "validating the local Pixi pack cache",
        );
        self.pack_cache.lookup(request, progress_sink)
    }

    pub fn probe_local_pixi(
        &mut self,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<String> {
        progress(
            progress_sink,
            RemoteStage::CheckLocalTool,
            "checking local Pixi for environment packing",
        );
        let mut command = CommandSpec::new("pixi");
        command.args = vec!["--version".into()];
        let output = run_checked(&mut self.command_runner, &command, "checking local Pixi")?;
        let version = std::str::from_utf8(&output.stdout)
            .context("local Pixi version is not UTF-8")?
            .trim();
        ensure!(!version.is_empty(), "local Pixi returned an empty version");
        Ok(version.to_owned())
    }

    pub fn probe_repository(
        &mut self,
        target: &SshTarget,
        repository: &Path,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteRepositoryState> {
        progress(
            progress_sink,
            RemoteStage::ProbeRepository,
            format!("checking remote checkout {}", repository.display()),
        );
        probe_remote_repository(&mut self.command_runner, target, repository)
    }

    pub fn probe_run_paths(
        &mut self,
        target: &SshTarget,
        repository: &Path,
        config: Option<&Path>,
        build_dir: Option<&Path>,
        data: Option<&Path>,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<()> {
        progress(
            progress_sink,
            RemoteStage::ValidateInvocation,
            "checking explicit remote config, build, and dataset paths",
        );
        probe_remote_run_paths(
            &mut self.command_runner,
            target,
            repository,
            config,
            build_dir,
            data,
        )
    }

    pub fn remote_pack_cached(
        &mut self,
        target: &SshTarget,
        key: &PackCacheKey,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<bool> {
        progress(
            progress_sink,
            RemoteStage::CheckRemotePack,
            "checking remote Pixi pack cache",
        );
        remote_pack_exists(&mut self.command_runner, target, key)
    }

    pub fn cleanup_job(
        &mut self,
        target: &SshTarget,
        run_id: &RemoteRunId,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<()> {
        progress(
            progress_sink,
            RemoteStage::CleanupJob,
            format!("cleaning up remote run {}", run_id.as_str()),
        );
        cleanup_remote_job(&mut self.command_runner, target, run_id)
    }

    pub fn execute<T>(
        &mut self,
        request: &RemoteRunRequest<T>,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteRunOutcome>
    where
        T: Serialize,
    {
        validate_request(request)?;
        let compatibility = self.probe(&request.target, progress_sink)?;
        self.execute_preprobed(request, compatibility, progress_sink)
    }

    pub fn execute_preprobed<T>(
        &mut self,
        request: &RemoteRunRequest<T>,
        compatibility: RemoteCompatibility,
        progress_sink: &mut impl ProgressSink,
    ) -> anyhow::Result<RemoteRunOutcome>
    where
        T: Serialize,
    {
        validate_request(request)?;
        compatibility.ensure_compatible(
            &request.pack.target_platform,
            if request.pack.cuda_required {
                &request.pack.cuda_profile
            } else {
                "cpu"
            },
        )?;

        let pack_key = request.pack.key_inputs()?.cache_key();

        let remote_pack_cache_hit =
            self.remote_pack_cached(&request.target, &pack_key, progress_sink)?;
        let local_pack = if remote_pack_cache_hit {
            None
        } else {
            let packed =
                self.pack_cache
                    .ensure(&mut self.command_runner, &request.pack, progress_sink)?;
            progress(
                progress_sink,
                RemoteStage::UploadPack,
                format!("uploading Pixi pack {}", packed.key),
            );
            upload_pack(&mut self.command_runner, &request.target, &packed)?;
            progress(
                progress_sink,
                RemoteStage::UnpackPack,
                "unpacking remote Pixi environment",
            );
            unpack_pack(&mut self.command_runner, &request.target, &packed)?;
            Some(packed)
        };

        progress(
            progress_sink,
            RemoteStage::PrepareJob,
            format!("preparing remote run {}", request.run_id.as_str()),
        );
        prepare_job(&mut self.command_runner, &request.target, &request.run_id)?;

        let runner = ArtifactDescriptor::from_file(&request.runner_binary)?;
        progress(
            progress_sink,
            RemoteStage::UploadRunner,
            "uploading runner binary",
        );
        upload_job_file(
            &mut self.command_runner,
            &request.target,
            &request.run_id,
            "runner",
            &request.runner_binary,
            &runner,
        )?;

        let input = request
            .input_archive
            .as_deref()
            .map(ArtifactDescriptor::from_file)
            .transpose()?;
        if let (Some(path), Some(descriptor)) = (&request.input_archive, &input) {
            progress(
                progress_sink,
                RemoteStage::UploadInput,
                "uploading run input archive",
            );
            upload_job_file(
                &mut self.command_runner,
                &request.target,
                &request.run_id,
                "input.tar",
                path,
                descriptor,
            )?;
        }

        let handshake_request = HandshakeRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            client_version: request.client_version.clone(),
            plan_version: request.plan_version,
            pack_key: pack_key.clone(),
            runner,
        };
        progress(
            progress_sink,
            RemoteStage::Handshake,
            "checking remote runner compatibility",
        );
        let handshake = handshake(
            &mut self.command_runner,
            &request.target,
            &request.run_id,
            &pack_key,
            &handshake_request,
        )?;
        handshake.validate(&handshake_request)?;

        let plan = RemotePlan {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            plan_version: request.plan_version,
            run_id: request.run_id.clone(),
            pack_key: pack_key.clone(),
            input,
            payload: &request.payload,
        };
        progress(
            progress_sink,
            RemoteStage::Execute,
            format!("running {} remotely", request.run_id.as_str()),
        );
        let envelope = run_remote(
            &mut self.command_runner,
            &request.target,
            &request.run_id,
            &pack_key,
            &plan,
        )?;
        envelope.validate(&request.run_id)?;

        let local_result_archive = if let Some(descriptor) = &envelope.result {
            progress(
                progress_sink,
                RemoteStage::DownloadResult,
                "downloading result archive",
            );
            download_result(
                &mut self.command_runner,
                &request.target,
                &request.run_id,
                &request.result_archive,
                descriptor,
            )?;
            Some(request.result_archive.clone())
        } else {
            None
        };

        progress(
            progress_sink,
            RemoteStage::Complete,
            format!("remote run {} finished", request.run_id.as_str()),
        );
        Ok(RemoteRunOutcome {
            compatibility,
            pack_key,
            local_pack,
            remote_pack_cache_hit,
            handshake,
            envelope,
            local_result_archive,
        })
    }
}

fn validate_request<T>(request: &RemoteRunRequest<T>) -> anyhow::Result<()> {
    ensure!(
        request.runner_binary.is_file(),
        "runner binary does not exist: {}",
        request.runner_binary.display()
    );
    ensure!(
        !request.result_archive.exists(),
        "result archive already exists: {}",
        request.result_archive.display()
    );
    if let Some(input) = &request.input_archive {
        ensure!(
            input.is_file(),
            "remote input archive does not exist: {}",
            input.display()
        );
    }
    Ok(())
}

pub fn probe_compatibility(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
) -> anyhow::Result<RemoteCompatibility> {
    let command = ssh_script(target, PROBE_SCRIPT, &[], CommandInput::Null);
    let output = run_checked(runner, &command, "probing remote compatibility")?;
    parse_probe(&output.stdout)
}

pub fn probe_remote_repository(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    repository: &Path,
) -> anyhow::Result<RemoteRepositoryState> {
    let repository = repository
        .to_str()
        .context("remote repository path is not valid UTF-8")?;
    ensure!(
        !repository.is_empty() && !repository.chars().any(char::is_control),
        "remote repository path cannot be empty or contain control characters"
    );
    const SCRIPT: &str = r#"set -eu
repo=$1
[ -d "$repo" ] || { printf 'remote repository is not a directory: %s\n' "$repo" >&2; exit 3; }
for marker in pixi.toml pixi.lock CMakeLists.txt .gitmodules rust/Cargo.toml src; do
  [ -e "$repo/$marker" ] || { printf 'remote checkout is missing %s\n' "$marker" >&2; exit 3; }
done
for submodule in duckdb cucascade substrait; do
  [ -e "$repo/$submodule/.git" ] || {
    printf 'remote submodule %s is not initialized; run git submodule update --init --recursive\n' "$submodule" >&2
    exit 3
  }
done
manifest=$(sha256sum "$repo/pixi.toml" | cut -d ' ' -f 1)
lock=$(sha256sum "$repo/pixi.lock" | cut -d ' ' -f 1)
commit=$(git --no-optional-locks -C "$repo" rev-parse HEAD)
dirty=clean
root_status=$(git --no-optional-locks -C "$repo" status --porcelain=v1 --untracked-files=normal --ignore-submodules=dirty)
[ -z "$root_status" ] || dirty=dirty
for submodule in duckdb cucascade substrait; do
  sub_status=$(git --no-optional-locks -C "$repo/$submodule" status --porcelain=v1 --untracked-files=normal)
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    if [ "$submodule" = duckdb ] &&
       [ "$line" = "?? CMakePresets.json" ] &&
       [ -L "$repo/duckdb/CMakePresets.json" ] &&
       [ "$(readlink -f "$repo/duckdb/CMakePresets.json")" = "$(readlink -f "$repo/cmake/CMakePresets.json")" ]; then
      continue
    fi
    dirty=dirty
  done <<EOF
$sub_status
EOF
done
printf 'ready\n%s\n%s\n%s\n%s\n' "$manifest" "$lock" "$commit" "$dirty"
"#;
    let command = ssh_script(target, SCRIPT, &[repository.to_owned()], CommandInput::Null);
    let output = run_checked(runner, &command, "checking remote Sirius checkout")?;
    let response =
        std::str::from_utf8(&output.stdout).context("remote checkout probe returned non-UTF-8")?;
    let mut lines = response.lines();
    ensure!(
        lines.next() == Some("ready"),
        "remote checkout probe returned an invalid response"
    );
    let manifest_sha256 = Sha256Digest::parse(
        lines
            .next()
            .context("remote checkout probe omitted pixi.toml checksum")?,
    )
    .context("remote checkout probe returned an invalid pixi.toml checksum")?;
    let lock_sha256 = Sha256Digest::parse(
        lines
            .next()
            .context("remote checkout probe omitted pixi.lock checksum")?,
    )
    .context("remote checkout probe returned an invalid pixi.lock checksum")?;
    let git_commit = lines
        .next()
        .context("remote checkout probe omitted Git commit")?
        .to_owned();
    ensure!(
        matches!(git_commit.len(), 40 | 64)
            && git_commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "remote checkout probe returned an invalid Git commit"
    );
    let git_dirty = match lines
        .next()
        .context("remote checkout probe omitted Git status")?
    {
        "clean" => false,
        "dirty" => true,
        _ => bail!("remote checkout probe returned an invalid Git status"),
    };
    ensure!(
        lines.next().is_none(),
        "remote checkout probe returned unexpected extra output"
    );
    Ok(RemoteRepositoryState {
        manifest_sha256,
        lock_sha256,
        git_commit,
        git_dirty,
    })
}

fn probe_remote_run_paths(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    repository: &Path,
    config: Option<&Path>,
    build_dir: Option<&Path>,
    data: Option<&Path>,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
repo=$1
config=$2
build=$3
data=$4
resolve() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$repo" "$1" ;;
  esac
}
if [ -n "$config" ]; then
  path=$(resolve "$config")
  [ -f "$path" ] || { printf 'remote config is not a file: %s\n' "$path" >&2; exit 3; }
fi
if [ -n "$build" ]; then
  path=$(resolve "$build")
  [ -d "$path" ] || { printf 'remote build directory is missing: %s\n' "$path" >&2; exit 3; }
  [ -x "$path/duckdb" ] || { printf 'remote DuckDB executable is missing: %s/duckdb\n' "$path" >&2; exit 3; }
  [ -f "$path/extension/sirius/sirius.duckdb_extension" ] || {
    printf 'remote Sirius extension is missing below: %s\n' "$path" >&2
    exit 3
  }
fi
if [ -n "$data" ]; then
  path=$(resolve "$data")
  [ -e "$path" ] || { printf 'remote dataset is missing: %s\n' "$path" >&2; exit 3; }
fi
printf 'ready\n'
"#;
    let arguments = [
        remote_path_argument(repository, "remote repository")?,
        optional_remote_path_argument(config, "remote config")?,
        optional_remote_path_argument(build_dir, "remote build directory")?,
        optional_remote_path_argument(data, "remote dataset")?,
    ];
    let command = ssh_script(target, SCRIPT, &arguments, CommandInput::Null);
    let output = run_checked(runner, &command, "checking explicit remote run paths")?;
    ensure!(
        output.stdout == b"ready\n",
        "remote run-path probe returned an invalid response"
    );
    Ok(())
}

fn optional_remote_path_argument(path: Option<&Path>, label: &str) -> anyhow::Result<String> {
    path.map(|path| remote_path_argument(path, label))
        .unwrap_or_else(|| Ok(String::new()))
}

fn remote_path_argument(path: &Path, label: &str) -> anyhow::Result<String> {
    let value = path
        .to_str()
        .with_context(|| format!("{label} path is not valid UTF-8"))?;
    ensure!(
        !value.chars().any(char::is_control),
        "{label} path cannot contain control characters"
    );
    Ok(value.to_owned())
}

fn parse_probe(output: &[u8]) -> anyhow::Result<RemoteCompatibility> {
    let output = std::str::from_utf8(output).context("remote probe returned non-UTF-8 output")?;
    let mut lines = output.lines();
    ensure!(
        lines.next() == Some(PROBE_MARKER),
        "remote probe returned an unsupported response"
    );
    let os = lines.next().context("remote probe omitted OS")?.to_owned();
    let architecture = lines
        .next()
        .context("remote probe omitted architecture")?
        .to_owned();
    let glibc_version = optional_line(lines.next().context("remote probe omitted glibc field")?);
    let nvidia_driver_version = optional_line(
        lines
            .next()
            .context("remote probe omitted NVIDIA driver field")?,
    );
    let nvidia_cuda_version = optional_line(
        lines
            .next()
            .context("remote probe omitted NVIDIA CUDA field")?,
    );
    let nvidia_compute_capabilities = lines
        .next()
        .context("remote probe omitted NVIDIA compute-capability field")?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    let flock_available = match lines
        .next()
        .context("remote probe omitted flock availability")?
    {
        "yes" => true,
        "no" => false,
        _ => bail!("remote probe returned invalid flock availability"),
    };
    ensure!(
        lines.all(str::is_empty),
        "remote probe returned unexpected extra output"
    );
    Ok(RemoteCompatibility {
        os,
        architecture,
        glibc_version,
        nvidia_driver_version,
        nvidia_cuda_version,
        nvidia_compute_capabilities,
        flock_available,
    })
}

fn optional_line(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn remote_pack_exists(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    key: &PackCacheKey,
) -> anyhow::Result<bool> {
    const SCRIPT: &str = r#"set -eu
key=$1
case "$key" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#key}" -eq 64 ] || exit 2
dir="$HOME/.cache/sirius-runner/v1/packs/$key"
recorded_key=$(sed -n '1p' "$dir/READY" 2>/dev/null || true)
recorded_digest=$(sed -n '2p' "$dir/READY" 2>/dev/null || true)
case "$recorded_digest" in ''|*[!0-9a-f]*) recorded_digest=;; esac
if [ "$recorded_key" = "$key" ] &&
   [ "${#recorded_digest}" -eq 64 ] &&
   [ "$(sha256sum "$dir/environment.sh" 2>/dev/null | cut -d ' ' -f 1)" = "$recorded_digest" ] &&
   [ -f "$dir/activate.sh" ] &&
   [ -x "$dir/env/bin/python" ] &&
   [ -x "$dir/env/bin/cmake" ] &&
   [ -x "$dir/env/bin/make" ]; then
  printf 'hit\n'
else
  printf 'miss\n'
fi
"#;
    let command = ssh_script(target, SCRIPT, &[key.as_str()], CommandInput::Null);
    let output = run_checked(runner, &command, "checking remote Pixi pack cache")?;
    match output.stdout.as_slice() {
        b"hit\n" => Ok(true),
        b"miss\n" => Ok(false),
        _ => bail!("remote Pixi pack probe returned an invalid response"),
    }
}

fn upload_pack(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    pack: &PackedEnvironment,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
key=$1
digest=$2
case "$key" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#key}" -eq 64 ] || exit 2
command -v flock >/dev/null 2>&1 || { printf 'remote host is missing flock\n' >&2; exit 3; }
slot_dir="/tmp/sirius-runner-$(id -u)"
mkdir -p "$slot_dir"
[ ! -L "$slot_dir" ] && [ "$(stat -c %u "$slot_dir")" = "$(id -u)" ] || {
  printf 'remote benchmark lock directory is not owned by the current user\n' >&2
  exit 3
}
chmod 700 "$slot_dir"
exec 9>"$slot_dir/benchmark.lock"
printf 'Waiting for the remote benchmark slot before uploading the Pixi pack\n' >&2
started=$(date +%s)
last=$started
while ! flock -n 9; do
  now=$(date +%s)
  if [ $((now - last)) -ge 10 ]; then
    printf 'Still waiting for the remote benchmark slot (%ss)\n' "$((now - started))" >&2
    last=$now
  fi
  sleep 1
done
dir="$HOME/.cache/sirius-runner/v1/packs/$key"
mkdir -p "$dir"
tmp="$dir/environment.sh.partial.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM
cat > "$tmp"
actual=$(sha256sum "$tmp" | cut -d ' ' -f 1)
[ "$actual" = "$digest" ] || { printf 'pack checksum mismatch\n' >&2; exit 3; }
chmod 700 "$tmp"
mv -f "$tmp" "$dir/environment.sh"
trap - EXIT HUP INT TERM
"#;
    let command = ssh_script(
        target,
        SCRIPT,
        &[pack.key.as_str(), pack.archive_sha256.as_hex()],
        CommandInput::File(pack.archive_path.clone()),
    );
    run_checked_live(runner, &command, "uploading Pixi pack")?;
    Ok(())
}

fn unpack_pack(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    pack: &PackedEnvironment,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
key=$1
digest=$2
case "$key" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#key}" -eq 64 ] || exit 2
command -v flock >/dev/null 2>&1 || { printf 'remote host is missing flock\n' >&2; exit 3; }
slot_dir="/tmp/sirius-runner-$(id -u)"
mkdir -p "$slot_dir"
[ ! -L "$slot_dir" ] && [ "$(stat -c %u "$slot_dir")" = "$(id -u)" ] || {
  printf 'remote benchmark lock directory is not owned by the current user\n' >&2
  exit 3
}
chmod 700 "$slot_dir"
exec 9>"$slot_dir/benchmark.lock"
printf 'Waiting for the remote benchmark slot before unpacking the Pixi environment\n' >&2
started=$(date +%s)
last=$started
while ! flock -n 9; do
  now=$(date +%s)
  if [ $((now - last)) -ge 10 ]; then
    printf 'Still waiting for the remote benchmark slot (%ss)\n' "$((now - started))" >&2
    last=$now
  fi
  sleep 1
done
dir="$HOME/.cache/sirius-runner/v1/packs/$key"
[ -x "$dir/environment.sh" ] || exit 3
recorded_key=$(sed -n '1p' "$dir/READY" 2>/dev/null || true)
recorded_digest=$(sed -n '2p' "$dir/READY" 2>/dev/null || true)
if [ "$recorded_key" = "$key" ] &&
   [ "$recorded_digest" = "$digest" ] &&
   [ "$(sha256sum "$dir/environment.sh" | cut -d ' ' -f 1)" = "$digest" ] &&
   [ -f "$dir/activate.sh" ] &&
   [ -x "$dir/env/bin/python" ] &&
   [ -x "$dir/env/bin/cmake" ] &&
   [ -x "$dir/env/bin/make" ]; then
  exit 0
fi
rm -rf -- "$dir/env"
rm -f -- "$dir/activate.sh" "$dir/READY"
(cd "$dir" && ./environment.sh)
printf '%s\n%s\n' "$key" "$digest" > "$dir/READY.partial"
mv -f "$dir/READY.partial" "$dir/READY"
"#;
    let command = ssh_script(
        target,
        SCRIPT,
        &[pack.key.as_str(), pack.archive_sha256.as_hex()],
        CommandInput::Null,
    );
    run_checked_live(runner, &command, "unpacking remote Pixi environment")?;
    Ok(())
}

fn prepare_job(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
id=$1
case "$id" in ''|.|..|*[!A-Za-z0-9._-]*) exit 2;; esac
job="$HOME/.cache/sirius-runner/v1/jobs/$id"
base="$HOME/.cache/sirius-runner/v1"
mkdir -p "$base/jobs"
chmod 700 "$base" "$base/jobs"
mkdir "$job" || { printf 'remote run ID already exists\n' >&2; exit 3; }
chmod 700 "$job"
"#;
    let command = ssh_script(
        target,
        SCRIPT,
        &[run_id.as_str().to_owned()],
        CommandInput::Null,
    );
    run_checked(runner, &command, "preparing remote job")?;
    Ok(())
}

fn cleanup_remote_job(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
id=$1
case "$id" in ''|.|..|*[!A-Za-z0-9._-]*) exit 2;; esac
jobs="$HOME/.cache/sirius-runner/v1/jobs"
job="$jobs/$id"
[ -d "$job" ] || { printf 'remote job does not exist\n' >&2; exit 3; }
rm -rf -- "$job"
[ ! -e "$job" ] || { printf 'remote job cleanup failed\n' >&2; exit 3; }
"#;
    let command = ssh_script(
        target,
        SCRIPT,
        &[run_id.as_str().to_owned()],
        CommandInput::Null,
    );
    run_checked(runner, &command, "cleaning up remote job")?;
    Ok(())
}

fn upload_job_file(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
    name: &str,
    source: &Path,
    descriptor: &ArtifactDescriptor,
) -> anyhow::Result<()> {
    ensure!(
        matches!(name, "runner" | "input.tar"),
        "unsupported remote job file"
    );
    const SCRIPT: &str = r#"set -eu
id=$1
name=$2
digest=$3
case "$id" in ''|.|..|*[!A-Za-z0-9._-]*) exit 2;; esac
case "$name" in runner|input.tar) ;; *) exit 2;; esac
job="$HOME/.cache/sirius-runner/v1/jobs/$id"
[ -d "$job" ] || exit 3
tmp="$job/$name.partial.$$"
trap 'rm -f "$tmp"' EXIT HUP INT TERM
cat > "$tmp"
actual=$(sha256sum "$tmp" | cut -d ' ' -f 1)
[ "$actual" = "$digest" ] || { printf 'artifact checksum mismatch\n' >&2; exit 3; }
if [ "$name" = runner ]; then chmod 700 "$tmp"; else chmod 600 "$tmp"; fi
mv -f "$tmp" "$job/$name"
trap - EXIT HUP INT TERM
"#;
    let command = ssh_script(
        target,
        SCRIPT,
        &[
            run_id.as_str().to_owned(),
            name.to_owned(),
            descriptor.sha256.as_hex(),
        ],
        CommandInput::File(source.to_owned()),
    );
    run_checked(runner, &command, &format!("uploading remote {name}"))?;
    Ok(())
}

fn handshake(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
    pack_key: &PackCacheKey,
    request: &HandshakeRequest,
) -> anyhow::Result<HandshakeResponse> {
    let command = worker_command(
        target,
        run_id,
        pack_key,
        "handshake",
        serde_json::to_vec(request)?,
    );
    let output = run_checked(runner, &command, "performing remote handshake")?;
    serde_json::from_slice(&output.stdout).context("parsing remote handshake")
}

fn run_remote<T>(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
    pack_key: &PackCacheKey,
    plan: &RemotePlan<T>,
) -> anyhow::Result<ResultEnvelope<ArtifactDescriptor>>
where
    T: Serialize,
{
    let command = worker_command(target, run_id, pack_key, "run", serde_json::to_vec(plan)?);
    let output = run_checked_live(runner, &command, "running remote benchmark")?;
    serde_json::from_slice(&output.stdout).context("parsing remote result envelope")
}

fn worker_command(
    target: &SshTarget,
    run_id: &RemoteRunId,
    pack_key: &PackCacheKey,
    action: &str,
    input: Vec<u8>,
) -> CommandSpec {
    const SCRIPT: &str = r#"set -eu
key=$1
id=$2
action=$3
case "$key" in ''|*[!0-9a-f]*) exit 2;; esac
[ "${#key}" -eq 64 ] || exit 2
case "$id" in ''|.|..|*[!A-Za-z0-9._-]*) exit 2;; esac
case "$action" in handshake|run) ;; *) exit 2;; esac
base="$HOME/.cache/sirius-runner/v1"
pack="$base/packs/$key"
job="$base/jobs/$id"
[ -f "$pack/READY" ] && [ -x "$job/runner" ] || exit 3
. "$pack/activate.sh" 1>&2
export SIRIUS_REMOTE_JOB_DIR="$job"
export SIRIUS_REMOTE_PACK_KEY="$key"
"$job/runner" __remote-worker "$action" &
worker=$!
trap 'kill -TERM "$worker" 2>/dev/null || true' HUP INT TERM
set +e
wait "$worker"
status=$?
set -e
trap - HUP INT TERM
exit "$status"
"#;
    ssh_script(
        target,
        SCRIPT,
        &[
            pack_key.as_str(),
            run_id.as_str().to_owned(),
            action.to_owned(),
        ],
        CommandInput::Bytes(input),
    )
}

fn download_result(
    runner: &mut impl CommandRunner,
    target: &SshTarget,
    run_id: &RemoteRunId,
    destination: &Path,
    expected: &ArtifactDescriptor,
) -> anyhow::Result<()> {
    const SCRIPT: &str = r#"set -eu
id=$1
case "$id" in ''|.|..|*[!A-Za-z0-9._-]*) exit 2;; esac
cat -- "$HOME/.cache/sirius-runner/v1/jobs/$id/result.tar"
"#;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating result directory {}", parent.display()))?;
    }
    let temp = destination.with_extension(format!(
        "partial-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let cleanup = RemoveOnDrop::file(temp.clone());
    let mut command = ssh_script(
        target,
        SCRIPT,
        &[run_id.as_str().to_owned()],
        CommandInput::Null,
    );
    command.stdout = CommandOutputTarget::File(temp.clone());
    run_checked_live(runner, &command, "downloading remote result")?;

    let actual = ArtifactDescriptor::from_file(&temp)?;
    ensure!(
        &actual == expected,
        "downloaded result archive does not match the remote result envelope"
    );
    fs::hard_link(&temp, destination).with_context(|| {
        format!(
            "publishing result archive {} to {}",
            temp.display(),
            destination.display()
        )
    })?;
    drop(cleanup);
    Ok(())
}

fn ssh_script(
    target: &SshTarget,
    script: &str,
    arguments: &[String],
    input: CommandInput,
) -> CommandSpec {
    let mut remote_arguments = vec![
        "-c".to_owned(),
        format!("umask 077\n{script}"),
        "sirius-runner-remote".to_owned(),
    ];
    remote_arguments.extend_from_slice(arguments);
    let remote_command = render_remote_command("sh", &remote_arguments);

    let mut command = CommandSpec::new("ssh");
    command.args = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "ConnectTimeout=15".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=4".into(),
        "-T".into(),
        "--".into(),
        target.as_str().into(),
        remote_command.into(),
    ];
    command.stdin = input;
    command
}

fn render_remote_command(program: &str, arguments: &[String]) -> String {
    std::iter::once(program)
        .chain(arguments.iter().map(String::as_str))
        .map(posix_shell_quote)
        .collect::<Vec<_>>()
        .join(" ")
}

fn posix_shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, process::Command};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn pack_cache_key_is_deterministic_and_covers_every_input() {
        let inputs = sample_key_inputs();
        let key = inputs.cache_key();
        assert_eq!(key, inputs.clone().cache_key());
        assert_eq!(
            key.as_str(),
            "97063be93c5e19412810ce93e243ddd3c87e280e635ac1e8f5b480bdf02560cf"
        );

        let variants = [
            PackKeyInputs {
                manifest_digest: Sha256Digest::from_bytes(b"other manifest"),
                ..inputs.clone()
            },
            PackKeyInputs {
                lock_digest: Sha256Digest::from_bytes(b"other lock"),
                ..inputs.clone()
            },
            PackKeyInputs {
                environment: "other".to_owned(),
                ..inputs.clone()
            },
            PackKeyInputs {
                target_platform: "linux-aarch64".to_owned(),
                ..inputs.clone()
            },
            PackKeyInputs {
                cuda_profile: "cuda12".to_owned(),
                ..inputs.clone()
            },
            PackKeyInputs {
                pixi_pack_version: "9.9.9".to_owned(),
                ..inputs.clone()
            },
        ];
        for variant in variants {
            assert_ne!(key, variant.cache_key());
        }
    }

    #[test]
    fn digest_deserialization_rejects_malformed_values() {
        assert!(serde_json::from_str::<Sha256Digest>("\"abcd\"").is_err());
        assert!(serde_json::from_str::<Sha256Digest>(&format!("\"{}\"", "z".repeat(64))).is_err());
        let digest = Sha256Digest::from_bytes(b"value");
        assert_eq!(
            serde_json::from_str::<Sha256Digest>(&serde_json::to_string(&digest).unwrap()).unwrap(),
            digest
        );
    }

    #[test]
    fn identifiers_reject_paths_and_ssh_option_injection() {
        assert!(RemoteRunId::new("../escape").is_err());
        assert!(serde_json::from_str::<RemoteRunId>("\"../escape\"").is_err());
        assert!(SshTarget::new("-oProxyCommand=bad").is_err());
        assert!(SshTarget::new("host;touch-pwned").is_err());
        assert!(SshTarget::new("developer@[2001:db8::1]").is_ok());
    }

    #[test]
    fn remote_command_quotes_shell_metacharacters() {
        let temp = TempDir::new().unwrap();
        let marker = temp.path().join("injected");
        let value = format!("a'; touch {}; printf 'pwned", marker.display());
        let command = render_remote_command("printf", &["%s".to_owned(), value.clone()]);
        let output = Command::new("sh").arg("-c").arg(command).output().unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, value.as_bytes());
        assert!(!marker.exists());
    }

    #[test]
    fn ssh_commands_use_noninteractive_trust_and_liveness_options() {
        let target = SshTarget::new("example.test").unwrap();
        let command = ssh_script(&target, "true", &[], CommandInput::Null);
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>();

        for option in [
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ConnectTimeout=15",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=4",
        ] {
            assert!(args.iter().any(|arg| arg == option));
        }
        assert!(
            args.last()
                .is_some_and(|argument| argument.contains("umask 077"))
        );
    }

    #[test]
    fn parses_typed_compatibility_probe() {
        let probe = parse_probe(
            b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n570.86.15\n12.9\n8.0\nyes\n",
        )
        .unwrap();
        assert_eq!(
            probe,
            RemoteCompatibility {
                os: "Linux".to_owned(),
                architecture: "x86_64".to_owned(),
                glibc_version: Some("glibc 2.39".to_owned()),
                nvidia_driver_version: Some("570.86.15".to_owned()),
                nvidia_cuda_version: Some("12.9".to_owned()),
                nvidia_compute_capabilities: vec!["8.0".to_owned()],
                flock_available: true,
            }
        );
        probe.ensure_compatible("linux-64", "cuda12").unwrap();
        assert!(probe.ensure_compatible("linux-64", "cuda13").is_err());
        assert!(probe.ensure_compatible("linux-aarch64", "cuda13").is_err());

        let cpu_only =
            parse_probe(b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n\n\n\nyes\n").unwrap();
        cpu_only
            .ensure_compatible("linux-64", "cpu")
            .expect("NVIDIA fields are optional for CPU environments");
        assert!(cpu_only.ensure_compatible("linux-64", "cuda13").is_err());

        let no_flock =
            parse_probe(b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n\n\n\nno\n").unwrap();
        assert!(
            no_flock
                .ensure_compatible("linux-64", "cpu")
                .unwrap_err()
                .to_string()
                .contains("flock")
        );
    }

    #[test]
    fn compatibility_probe_script_runs_under_posix_shell() {
        let output = Command::new("sh")
            .args(["-c", PROBE_SCRIPT])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let probe = parse_probe(&output.stdout).unwrap();
        assert!(!probe.os.is_empty());
        assert!(!probe.architecture.is_empty());
    }

    #[test]
    fn explicit_remote_run_paths_are_checked_read_only() {
        let mut runner = FakeRunner::new([ProcessOutput::success("ready\n")]);
        let target = SshTarget::new("example.test").unwrap();

        probe_remote_run_paths(
            &mut runner,
            &target,
            Path::new("/srv/sirius"),
            Some(Path::new("config.yaml")),
            Some(Path::new("build/release")),
            Some(Path::new("/datasets/tpch")),
        )
        .unwrap();

        assert_eq!(runner.commands.len(), 1);
        assert_eq!(runner.commands[0].program, "ssh");
        assert_eq!(runner.commands[0].stdin, CommandInput::Null);
        let command = runner.commands[0].args.last().unwrap().to_string_lossy();
        assert!(command.contains("'config.yaml'"));
        assert!(command.contains("'build/release'"));
        assert!(command.contains("'/datasets/tpch'"));
    }

    #[test]
    fn successful_remote_job_cleanup_is_scoped_to_the_run_id() {
        let mut runner = FakeRunner::new([ProcessOutput::success(Vec::new())]);
        let target = SshTarget::new("example.test").unwrap();
        let run_id = RemoteRunId::new("run-123").unwrap();

        cleanup_remote_job(&mut runner, &target, &run_id).unwrap();

        assert_eq!(runner.commands.len(), 1);
        assert_eq!(runner.commands[0].program, "ssh");
        assert_eq!(runner.commands[0].stdin, CommandInput::Null);
        let command = runner.commands[0].args.last().unwrap().to_string_lossy();
        assert!(command.contains("'run-123'"));
        assert!(command.contains("jobs/$id"));
        assert!(command.contains("rm -rf -- \"$job\""));
    }

    #[test]
    fn pixi_pack_is_built_once_and_then_loaded_from_cache() {
        let fixture = Fixture::new();
        let mut runner = FakeRunner::new([ProcessOutput::success(Vec::new())]);
        let cache = PixiPackCache::new(fixture.cache.path());
        let mut events = Vec::new();

        let first = cache
            .ensure(&mut runner, &fixture.pack_request, &mut |event| {
                events.push(event)
            })
            .unwrap();
        let second = cache
            .ensure(&mut runner, &fixture.pack_request, &mut NoopProgress)
            .unwrap();

        assert!(!first.cache_hit);
        assert!(second.cache_hit);
        assert_eq!(first.key, second.key);
        assert_eq!(runner.commands.len(), 1);
        assert!(
            runner.commands[0]
                .args
                .iter()
                .any(|arg| arg == "--create-executable")
        );
        assert!(
            runner.commands[0]
                .args
                .windows(2)
                .any(|arguments| { arguments[0] == "--platform" && arguments[1] == "linux-64" })
        );
        assert!(
            events
                .iter()
                .any(|event| event.stage == RemoteStage::BuildLocalPack)
        );
    }

    #[test]
    fn corrupt_local_pack_is_rebuilt_under_the_cache_lock() {
        let fixture = Fixture::new();
        let cache = PixiPackCache::new(fixture.cache.path());
        let mut first_runner = FakeRunner::new([ProcessOutput::success(Vec::new())]);
        let first = cache
            .ensure(&mut first_runner, &fixture.pack_request, &mut NoopProgress)
            .unwrap();
        fs::write(&first.archive_path, b"corrupt").unwrap();
        assert!(
            cache
                .lookup(&fixture.pack_request, &mut NoopProgress)
                .is_err()
        );

        let mut second_runner = FakeRunner::new([ProcessOutput::success(Vec::new())]);
        let rebuilt = cache
            .ensure(&mut second_runner, &fixture.pack_request, &mut NoopProgress)
            .unwrap();

        assert!(!rebuilt.cache_hit);
        assert_eq!(
            fs::read(&rebuilt.archive_path).unwrap(),
            b"packed environment"
        );
        assert_eq!(second_runner.commands.len(), 1);
        assert!(fs::read_dir(fixture.cache.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".invalid-")
        }));
    }

    #[test]
    fn pixi_pack_request_rejects_named_cuda_platforms() {
        let fixture = Fixture::new();
        let mut request = fixture.pack_request;
        request.target_platform = "linux-64-cuda13".to_owned();

        let error = request.key_inputs().unwrap_err();

        assert!(error.to_string().contains("Pixi Pack platform"));
    }

    #[test]
    fn cache_miss_executes_upload_unpack_run_and_download_flow() {
        let fixture = Fixture::new();
        let pack_inputs = fixture.pack_request.key_inputs().unwrap();
        let pack_key = pack_inputs.cache_key();
        let runner_descriptor = ArtifactDescriptor::from_file(&fixture.runner).unwrap();
        let result_bytes = b"result archive".to_vec();
        let result_descriptor = ArtifactDescriptor {
            sha256: Sha256Digest::from_bytes(&result_bytes),
            bytes: result_bytes.len() as u64,
        };
        let handshake = HandshakeResponse {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            runner_version: "0.1.0".to_owned(),
            plan_version: 7,
            pack_key: pack_key.clone(),
            runner: runner_descriptor,
        };
        let envelope = ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: 1,
            run_id: RemoteRunId::new("run-123").unwrap(),
            status: RemoteRunStatus::Completed,
            result: Some(result_descriptor),
            error: None,
        };
        let responses = [
            ProcessOutput::success(
                b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n570.1\n13.2\n8.0\nyes\n"
                    .to_vec(),
            ),
            ProcessOutput::success(b"miss\n".to_vec()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(serde_json::to_vec(&handshake).unwrap()),
            ProcessOutput::success(serde_json::to_vec(&envelope).unwrap()),
            ProcessOutput::success(result_bytes.clone()),
        ];
        let fake = FakeRunner::new(responses);
        let cache = PixiPackCache::new(fixture.cache.path());
        let mut executor = RemoteExecutor::new(fake, cache);
        let request = fixture.remote_request("run-123", 7);
        let mut events = Vec::new();

        let outcome = executor
            .execute(&request, &mut |event| events.push(event))
            .unwrap();

        assert!(!outcome.remote_pack_cache_hit);
        assert!(outcome.local_pack.is_some());
        assert_eq!(fs::read(&request.result_archive).unwrap(), result_bytes);
        assert_eq!(executor.command_runner().commands.len(), 11);
        let global_slot_commands = executor
            .command_runner()
            .commands
            .iter()
            .filter(|command| {
                command
                    .args
                    .last()
                    .is_some_and(|argument| argument.to_string_lossy().contains("benchmark.lock"))
            })
            .count();
        assert_eq!(global_slot_commands, 2);
        assert!(
            events
                .iter()
                .any(|event| event.stage == RemoteStage::UploadPack)
        );
        assert!(
            events
                .iter()
                .any(|event| event.stage == RemoteStage::UnpackPack)
        );
        assert_eq!(events.last().unwrap().stage, RemoteStage::Complete);

        let payload = "value; touch should-not-run";
        assert!(
            executor
                .command_runner()
                .commands
                .iter()
                .filter(|command| command.program == "ssh")
                .all(|command| !command
                    .args
                    .last()
                    .unwrap()
                    .to_string_lossy()
                    .contains(payload))
        );
        let protocol_inputs = executor
            .command_runner()
            .commands
            .iter()
            .filter_map(|command| match &command.stdin {
                CommandInput::Bytes(bytes) => Some(bytes.as_slice()),
                CommandInput::Null | CommandInput::File(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(protocol_inputs.len(), 2);
        let sent_handshake: HandshakeRequest = serde_json::from_slice(protocol_inputs[0]).unwrap();
        assert_eq!(sent_handshake.protocol_version, REMOTE_PROTOCOL_VERSION);
        let sent_plan: RemotePlan<String> = serde_json::from_slice(protocol_inputs[1]).unwrap();
        assert_eq!(sent_plan.payload, payload);
        assert_eq!(sent_plan.input.unwrap().bytes, b"input bytes".len() as u64);
    }

    #[test]
    fn remote_pack_hit_skips_pack_upload_and_unpack() {
        let fixture = Fixture::new();
        let mut pack_runner = FakeRunner::new([ProcessOutput::success(Vec::new())]);
        let cache = PixiPackCache::new(fixture.cache.path());
        cache
            .ensure(&mut pack_runner, &fixture.pack_request, &mut NoopProgress)
            .unwrap();

        let pack_key = fixture.pack_request.key_inputs().unwrap().cache_key();
        let runner_descriptor = ArtifactDescriptor::from_file(&fixture.runner).unwrap();
        let handshake = HandshakeResponse {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            runner_version: "0.1.0".to_owned(),
            plan_version: 3,
            pack_key,
            runner: runner_descriptor,
        };
        let envelope: ResultEnvelope<ArtifactDescriptor> = ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: 1,
            run_id: RemoteRunId::new("cached-run").unwrap(),
            status: RemoteRunStatus::Failed,
            result: None,
            error: Some(RemoteFailure {
                code: "query_failed".to_owned(),
                message: "expected test failure".to_owned(),
            }),
        };
        let fake = FakeRunner::new([
            ProcessOutput::success(
                b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n570.1\n13.2\n8.0\nyes\n"
                    .to_vec(),
            ),
            ProcessOutput::success(b"hit\n".to_vec()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(serde_json::to_vec(&handshake).unwrap()),
            ProcessOutput::success(serde_json::to_vec(&envelope).unwrap()),
        ]);
        let mut executor = RemoteExecutor::new(fake, cache);
        let request = fixture.remote_request_without_input("cached-run", 3);

        let outcome = executor.execute(&request, &mut NoopProgress).unwrap();

        assert!(outcome.remote_pack_cache_hit);
        assert!(outcome.local_pack.is_none());
        assert!(outcome.local_result_archive.is_none());
        assert_eq!(executor.command_runner().commands.len(), 6);
        let remote_scripts = executor
            .command_runner()
            .commands
            .iter()
            .filter(|command| command.program == "ssh")
            .map(|command| command.args.last().unwrap().to_string_lossy())
            .collect::<Vec<_>>();
        assert!(
            remote_scripts
                .iter()
                .all(|script| !script.contains("environment.sh.partial"))
        );
    }

    #[test]
    fn protocol_mismatch_stops_before_remote_run() {
        let fixture = Fixture::new();
        let pack_key = fixture.pack_request.key_inputs().unwrap().cache_key();
        let runner_descriptor = ArtifactDescriptor::from_file(&fixture.runner).unwrap();
        let handshake = HandshakeResponse {
            protocol_version: REMOTE_PROTOCOL_VERSION + 1,
            runner_version: "0.1.0".to_owned(),
            plan_version: 3,
            pack_key,
            runner: runner_descriptor,
        };
        let fake = FakeRunner::new([
            ProcessOutput::success(
                b"sirius-runner-probe-v3\nLinux\nx86_64\nglibc 2.39\n570.1\n13.2\n8.0\nyes\n"
                    .to_vec(),
            ),
            ProcessOutput::success(b"hit\n".to_vec()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(Vec::new()),
            ProcessOutput::success(serde_json::to_vec(&handshake).unwrap()),
        ]);
        let mut executor = RemoteExecutor::new(fake, PixiPackCache::new(fixture.cache.path()));
        let request = fixture.remote_request_without_input("mismatch", 3);

        let error = executor.execute(&request, &mut NoopProgress).unwrap_err();

        assert!(error.to_string().contains("protocol version"));
        assert_eq!(executor.command_runner().commands.len(), 5);
    }

    #[test]
    fn failed_envelope_can_describe_a_partial_result_bundle() {
        let run_id = RemoteRunId::new("partial").unwrap();
        let envelope = ResultEnvelope {
            protocol_version: REMOTE_PROTOCOL_VERSION,
            result_version: REMOTE_RESULT_VERSION,
            run_id: run_id.clone(),
            status: RemoteRunStatus::Failed,
            result: Some(ArtifactDescriptor {
                sha256: Sha256Digest::from_bytes(b"partial bundle"),
                bytes: 14,
            }),
            error: Some(RemoteFailure {
                code: "query_failed".to_owned(),
                message: "query failed".to_owned(),
            }),
        };

        envelope.validate(&run_id).unwrap();
    }

    fn sample_key_inputs() -> PackKeyInputs {
        PackKeyInputs {
            manifest_digest: Sha256Digest::from_bytes(b"manifest"),
            lock_digest: Sha256Digest::from_bytes(b"lock"),
            environment: "default".to_owned(),
            target_platform: "linux-64".to_owned(),
            cuda_profile: "cuda13".to_owned(),
            pixi_pack_version: "0.7.2".to_owned(),
            format: PixiPackFormat::SelfExtractingShellV1,
        }
    }

    struct Fixture {
        _root: TempDir,
        cache: TempDir,
        pack_request: PixiPackRequest,
        runner: PathBuf,
        input: PathBuf,
        result: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let manifest = root.path().join("pixi.toml");
            let lock = root.path().join("pixi.lock");
            let runner = root.path().join("sirius-runner");
            let input = root.path().join("input.tar");
            let result = root.path().join("result.tar");
            fs::write(&manifest, "[workspace]\nname = \"test\"\n").unwrap();
            fs::write(&lock, "lock content").unwrap();
            fs::write(&runner, "runner bytes").unwrap();
            fs::write(&input, "input bytes").unwrap();
            Self {
                _root: root,
                cache,
                pack_request: PixiPackRequest {
                    manifest_path: manifest,
                    lock_path: lock,
                    environment: "default".to_owned(),
                    target_platform: "linux-64".to_owned(),
                    cuda_profile: "cuda13".to_owned(),
                    cuda_required: true,
                    pixi_pack_version: "0.7.2".to_owned(),
                    format: PixiPackFormat::SelfExtractingShellV1,
                },
                runner,
                input,
                result,
            }
        }

        fn remote_request(&self, id: &str, plan_version: u32) -> RemoteRunRequest<String> {
            RemoteRunRequest {
                target: SshTarget::new("developer@example.test").unwrap(),
                client_version: "0.1.0".to_owned(),
                pack: self.pack_request.clone(),
                runner_binary: self.runner.clone(),
                input_archive: Some(self.input.clone()),
                run_id: RemoteRunId::new(id).unwrap(),
                plan_version,
                payload: "value; touch should-not-run".to_owned(),
                result_archive: self.result.clone(),
            }
        }

        fn remote_request_without_input(
            &self,
            id: &str,
            plan_version: u32,
        ) -> RemoteRunRequest<String> {
            let mut request = self.remote_request(id, plan_version);
            request.input_archive = None;
            request
        }
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
            if command.program == "pixi" {
                let current_dir = command
                    .current_dir
                    .as_deref()
                    .context("fake pixi-pack command has no working directory")?;
                fs::write(current_dir.join("environment.sh"), b"packed environment")?;
            }

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
