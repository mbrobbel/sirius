use clap::Args;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Report system specs: GPU, CPU, RAM, disks and free space.
#[derive(Args)]
#[command(alias = "doctor")]
pub struct Specs;

impl Specs {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(Unimplemented("specs").into())
    }
}
