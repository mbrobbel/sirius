use std::{
    io::{self, Read, Write},
    process::{Child, ChildStdout, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, bail};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::{
    cancel,
    progress::{Reporter, Stage},
};

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

pub fn run(
    command: &mut Command,
    label: impl Into<String>,
    reporter: &mut impl Reporter,
) -> anyhow::Result<ExitStatus> {
    run_inner(command, label.into(), None, true, reporter)
}

pub fn run_with_timeout(
    command: &mut Command,
    label: impl Into<String>,
    timeout: Option<Duration>,
    reporter: &mut impl Reporter,
) -> anyhow::Result<ExitStatus> {
    run_inner(command, label.into(), timeout, false, reporter)
}

fn run_inner(
    command: &mut Command,
    label: String,
    timeout: Option<Duration>,
    relay_stdout: bool,
    reporter: &mut impl Reporter,
) -> anyhow::Result<ExitStatus> {
    if relay_stdout {
        command.stdout(Stdio::piped());
    }
    configure_process_group(command);
    reporter.detail(&format!("Command: {command:?}"))?;
    let mut stage = Stage::start(reporter, label.clone())?;
    let mut child = ManagedChild::spawn(command).with_context(|| format!("starting {label}"))?;
    let stdout_relay = relay_stdout
        .then(|| child.take_stdout().map(spawn_stdout_relay))
        .flatten();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => {
                child.terminate();
                break Err(error).with_context(|| format!("waiting for {label}"));
            }
        }
        if let Err(error) = cancel::check() {
            child.terminate();
            break Err(error).with_context(|| format!("cancelling {label}"));
        }
        if timeout.is_some_and(|timeout| stage.elapsed() >= timeout) {
            child.terminate();
            break Err(anyhow::anyhow!(
                "{label} exceeded its {} timeout",
                format_timeout(timeout.unwrap())
            ));
        }
        thread::sleep(POLL_INTERVAL);
        if let Err(error) = stage.heartbeat(reporter, HEARTBEAT_INTERVAL) {
            child.terminate();
            break Err(error.into());
        }
    };
    let relay_result = join_stdout_relay(stdout_relay);
    let status = status?;
    relay_result?;
    if status.success() {
        stage.complete(reporter)?;
    } else {
        bail!("{label} failed with {status}");
    }
    Ok(status)
}

pub(crate) fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
}

pub(crate) fn terminate_child_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(process_group) = i32::try_from(child.id())
            && process_group_exists(process_group)
        {
            signal_process_group(process_group, libc::SIGTERM);
            let deadline = Instant::now() + TERMINATION_GRACE;
            while Instant::now() < deadline && process_group_exists(process_group) {
                thread::sleep(Duration::from_millis(50));
            }
            if process_group_exists(process_group) {
                signal_process_group(process_group, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
}

struct ManagedChild {
    child: Child,
    reaped: bool,
    #[cfg(unix)]
    process_group: i32,
}

impl ManagedChild {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let child = command.spawn()?;
        #[cfg(unix)]
        let process_group = i32::try_from(child.id())
            .map_err(|_| io::Error::other("child process ID exceeds i32"))?;
        Ok(Self {
            child,
            reaped: false,
            #[cfg(unix)]
            process_group,
        })
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }

    fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        {
            if process_group_exists(self.process_group) {
                signal_process_group(self.process_group, libc::SIGTERM);
                let deadline = Instant::now() + TERMINATION_GRACE;
                while Instant::now() < deadline && process_group_exists(self.process_group) {
                    thread::sleep(Duration::from_millis(50));
                }
                if process_group_exists(self.process_group) {
                    signal_process_group(self.process_group, libc::SIGKILL);
                }
            }
        }
        #[cfg(not(unix))]
        {
            if !self.reaped {
                let _ = self.child.kill();
            }
        }
        if !self.reaped {
            let _ = self.child.wait();
            self.reaped = true;
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
fn signal_process_group(process_group: i32, signal: i32) {
    // Negative PIDs address the dedicated child process group created above.
    unsafe {
        libc::kill(-process_group, signal);
    }
}

#[cfg(unix)]
fn process_group_exists(process_group: i32) -> bool {
    let result = unsafe { libc::kill(-process_group, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn spawn_stdout_relay(mut stdout: ChildStdout) -> thread::JoinHandle<io::Result<()>> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = stdout.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            let stderr = io::stderr();
            let mut stderr = stderr.lock();
            stderr.write_all(&buffer[..count])?;
            stderr.flush()?;
        }
    })
}

fn join_stdout_relay(relay: Option<thread::JoinHandle<io::Result<()>>>) -> anyhow::Result<()> {
    relay
        .map(|relay| {
            relay
                .join()
                .map_err(|_| anyhow::anyhow!("child stdout relay panicked"))?
                .context("relaying child stdout to stderr")
        })
        .transpose()
        .map(|_| ())
}

fn format_timeout(timeout: Duration) -> String {
    if timeout.as_secs().is_multiple_of(60) {
        format!("{}m", timeout.as_secs() / 60)
    } else {
        format!("{}s", timeout.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_the_entire_process_group() {
        let temp = tempfile::tempdir().unwrap();
        let pid_file = temp.path().join("grandchild.pid");
        let script = format!(
            "sleep 60 & child=$!; printf '%s' \"$child\" > '{}'; wait",
            pid_file.display()
        );
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);

        let error = run_with_timeout(
            &mut command,
            "process-group test",
            Some(Duration::from_millis(300)),
            &mut progress,
        )
        .unwrap_err();

        assert!(error.to_string().contains("timeout"));
        let pid = fs::read_to_string(pid_file)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        assert!(
            !process_is_running(pid),
            "grandchild {pid} survived timeout"
        );
    }

    #[cfg(unix)]
    fn process_is_running(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| stat.rsplit_once(')').map(|(_, suffix)| suffix.to_owned()))
            .and_then(|suffix| suffix.split_whitespace().next().map(str::to_owned))
            .is_some_and(|state| state != "Z")
    }
}
