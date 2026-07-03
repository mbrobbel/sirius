use clap::Args;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Compare two stored runs.
#[derive(Args)]
pub struct Compare {
    pub run_a: String,
    pub run_b: String,
}

impl Compare {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(Unimplemented("compare").into())
    }
}
