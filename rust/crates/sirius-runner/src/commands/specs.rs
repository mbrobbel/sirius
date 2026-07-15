use clap::Args;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Report system specs: GPU, CPU, RAM, disks and free space.
#[derive(Args)]
#[command(alias = "doctor")]
pub struct Specs;

impl Specs {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(Unimplemented::new(
            "specs",
            "Probe and report this machine's hardware: GPU (name, memory, driver, CUDA
version), CPU model and core count, RAM, and every disk with total/free space,
including the filesystem backing the data root. The same probe fills the
`environments` row attached to every stored run (so results stay comparable
across machines) and provides the free-space numbers that make dataset
generation disk-aware. --json emits it machine-readable for CI and for agents
tuning engine configs (executor threads, memory limits) to the hardware.",
        )
        .into())
    }
}
