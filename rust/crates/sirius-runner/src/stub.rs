use std::{error, fmt};

/// A command that is part of the CLI surface but has no implementation yet.
#[derive(Debug)]
pub struct Unimplemented(pub &'static str);

impl fmt::Display for Unimplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`sirius-runner {}` is not implemented yet", self.0)
    }
}

impl error::Error for Unimplemented {}
