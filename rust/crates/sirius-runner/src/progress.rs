use std::{
    fmt,
    io::{self, Write},
    time::{Duration, Instant},
};

pub trait Reporter {
    fn status(&mut self, message: &str) -> io::Result<()>;
    fn detail(&mut self, message: &str) -> io::Result<()>;
}

/// Plain, line-oriented progress output.
///
/// The production constructor is tied to stderr so progress cannot corrupt
/// command results on stdout, including JSON output. It intentionally does not
/// depend on terminal detection or color support.
pub struct Progress<W = io::Stderr> {
    writer: W,
    verbose: bool,
    closed: bool,
}

impl Progress<io::Stderr> {
    pub fn stderr(verbosity: u8) -> Self {
        Self {
            writer: io::stderr(),
            verbose: verbosity > 0,
            closed: false,
        }
    }
}

impl<W: Write> Progress<W> {
    #[cfg(test)]
    pub(crate) fn with_writer(writer: W, verbosity: u8) -> Self {
        Self {
            writer,
            verbose: verbosity > 0,
            closed: false,
        }
    }

    /// Report user-visible progress.
    pub fn status(&mut self, message: impl fmt::Display) -> io::Result<()> {
        self.write_line(message)
    }

    /// Report additional detail requested with `--verbose`.
    pub fn detail(&mut self, message: impl fmt::Display) -> io::Result<()> {
        if self.verbose {
            self.write_line(message)?;
        }
        Ok(())
    }

    fn write_line(&mut self, message: impl fmt::Display) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let result = writeln!(self.writer, "{message}").and_then(|()| self.writer.flush());
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
        {
            self.closed = true;
            Ok(())
        } else {
            result
        }
    }
}

impl<W: Write> Reporter for Progress<W> {
    fn status(&mut self, message: &str) -> io::Result<()> {
        self.status(message)
    }

    fn detail(&mut self, message: &str) -> io::Result<()> {
        self.detail(message)
    }
}

pub struct Stage {
    label: String,
    started: Instant,
    last_heartbeat: Instant,
}

impl Stage {
    pub fn start(reporter: &mut impl Reporter, label: impl Into<String>) -> io::Result<Self> {
        let label = label.into();
        reporter.status(&label)?;
        let now = Instant::now();
        Ok(Self {
            label,
            started: now,
            last_heartbeat: now,
        })
    }

    pub fn heartbeat(
        &mut self,
        reporter: &mut impl Reporter,
        interval: Duration,
    ) -> io::Result<()> {
        if self.last_heartbeat.elapsed() >= interval {
            reporter.status(&format!(
                "Still {} ({})",
                self.label.to_lowercase(),
                format_duration(self.started.elapsed())
            ))?;
            self.last_heartbeat = Instant::now();
        }
        Ok(())
    }

    pub fn complete(self, reporter: &mut impl Reporter) -> io::Result<()> {
        reporter.status(&format!(
            "Completed {} ({})",
            self.label.to_lowercase(),
            format_duration(self.started.elapsed())
        ))
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_visible_without_a_tty_or_color() {
        let mut output = Vec::new();
        let mut progress = Progress::with_writer(&mut output, 0);

        progress.status("Preparing dataset").unwrap();

        assert_eq!(output, b"Preparing dataset\n");
        assert!(!output.contains(&0x1b));
    }

    #[test]
    fn details_require_verbose_output() {
        let mut normal_output = Vec::new();
        let mut normal = Progress::with_writer(&mut normal_output, 0);
        normal.detail("cache receipt matched").unwrap();
        assert!(normal_output.is_empty());

        let mut verbose_output = Vec::new();
        let mut verbose = Progress::with_writer(&mut verbose_output, 1);
        verbose.detail("cache receipt matched").unwrap();
        assert_eq!(verbose_output, b"cache receipt matched\n");
    }

    #[test]
    fn duration_format_is_compact() {
        assert_eq!(format_duration(Duration::from_millis(1250)), "1.2s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn a_closed_progress_stream_does_not_cancel_useful_work() {
        struct Closed;

        impl Write for Closed {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut progress = Progress::with_writer(Closed, 0);
        progress.status("first").unwrap();
        progress.status("second").unwrap();
        assert!(progress.closed);
    }
}
