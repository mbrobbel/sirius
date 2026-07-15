use std::{error, fmt};

/// A command that is part of the CLI surface but has no implementation yet.
/// `details` documents the intended behavior and how the command fits the
/// bigger picture, so running a stub explains the design.
#[derive(Debug)]
pub struct Unimplemented {
    command: &'static str,
    details: &'static str,
}

impl Unimplemented {
    pub fn new(command: &'static str, details: &'static str) -> Self {
        Self { command, details }
    }
}

impl fmt::Display for Unimplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "`sirius-runner {}` is not implemented yet.",
            self.command
        )?;
        writeln!(f)?;
        writeln!(f, "When implemented, it will:")?;
        write!(f, "{}", self.details)
    }
}

impl error::Error for Unimplemented {}
