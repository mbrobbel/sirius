use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};

use crate::progress::Reporter;
use crate::{cancel, progress};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub struct RunLock {
    file: File,
}

impl RunLock {
    pub fn acquire(reporter: &mut impl Reporter) -> anyhow::Result<Self> {
        let path = default_lock_path()?;
        Self::acquire_at(&path, reporter)
    }

    fn acquire_at(path: &Path, reporter: &mut impl Reporter) -> anyhow::Result<Self> {
        let parent = path
            .parent()
            .context("benchmark lock path has no parent directory")?;
        prepare_lock_directory(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("opening benchmark lock {}", path.display()))?;

        reporter.status(&format!(
            "Waiting for the per-user host benchmark slot ({})",
            path.display()
        ))?;
        let started = Instant::now();
        let mut last_heartbeat = started;
        loop {
            cancel::check()?;
            match file.try_lock() {
                Ok(()) => {
                    reporter.status(&format!(
                        "Acquired the benchmark slot ({})",
                        progress::format_duration(started.elapsed())
                    ))?;
                    return Ok(Self { file });
                }
                Err(TryLockError::WouldBlock) => {
                    if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                        reporter.status(&format!(
                            "Still waiting for the benchmark slot ({})",
                            progress::format_duration(started.elapsed())
                        ))?;
                        last_heartbeat = Instant::now();
                    }
                    thread::sleep(POLL_INTERVAL);
                }
                Err(TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("locking benchmark slot {}", path.display()));
                }
            }
        }
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(unix)]
fn default_lock_path() -> anyhow::Result<PathBuf> {
    let user_id = unsafe { libc::geteuid() };
    Ok(PathBuf::from("/tmp")
        .join(format!("sirius-runner-{user_id}"))
        .join("benchmark.lock"))
}

#[cfg(not(unix))]
fn default_lock_path() -> anyhow::Result<PathBuf> {
    let home = PathBuf::from(
        std::env::var_os("HOME").context("HOME is not set; cannot locate the benchmark lock")?,
    );
    Ok(home.join(".cache/sirius-runner/v1/locks/benchmark.lock"))
}

fn prepare_lock_directory(path: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("creating benchmark lock directory {}", path.display()))?;
    ensure!(
        path.is_dir(),
        "benchmark lock parent is not a directory: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("inspecting benchmark lock directory {}", path.display()))?;
        ensure!(
            metadata.file_type().is_dir() && metadata.uid() == unsafe { libc::geteuid() },
            "benchmark lock directory is not owned by the current user: {}",
            path.display()
        );
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).with_context(|| {
                format!(
                    "restricting benchmark lock directory permissions {}",
                    path.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("locks/benchmark.lock");
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);
        let first = RunLock::acquire_at(&path, &mut progress).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();

        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        drop(first);
        second.try_lock().unwrap();
        second.unlock().unwrap();
    }
}
