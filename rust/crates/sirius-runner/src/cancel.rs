use std::{
    error::Error,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("interrupted by user")
    }
}

impl Error for Interrupted {}

pub fn install() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let previous =
                unsafe { libc::signal(signal, handle_signal as *const () as libc::sighandler_t) };
            if previous == libc::SIG_ERR {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    Ok(())
}

pub fn check() -> anyhow::Result<()> {
    if INTERRUPTED.load(Ordering::Relaxed) {
        Err(Interrupted.into())
    } else {
        Ok(())
    }
}

pub fn is_interrupted(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<Interrupted>())
}

#[cfg(unix)]
extern "C" fn handle_signal(_signal: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interruption_errors_are_typed() {
        let error = anyhow::Error::new(Interrupted).context("running a benchmark");
        assert!(is_interrupted(&error));
    }
}
