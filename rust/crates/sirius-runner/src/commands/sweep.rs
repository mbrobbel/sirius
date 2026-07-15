use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Sweep benchmark runs across configs and dataset encodings.
#[derive(Subcommand)]
pub enum Sweep {
    /// Run a benchmark across the dimensions in a sweep file.
    Run {
        /// Benchmark (run configuration) to sweep.
        #[arg(long)]
        bench: String,
        /// Sweep dimensions file (engine config and dataset encoding axes).
        #[arg(long, value_name = "FILE")]
        dimensions: PathBuf,
    },
}

impl Sweep {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Run { .. } => Unimplemented::new(
                "sweep run",
                "Expand a dimensions file (engine-config knobs like thread counts and memory
fractions, dataset compression/encodings, run modes) into a matrix of derived
benchmarks, run each through the bench-run machinery — generating dataset
variants as needed — and store all runs under one sweep id with a summary of
the best-performing configurations. Built for agent-driven config exploration:
an agent proposes dimensions, reads the stored outcomes, and iterates.",
            ),
        }
        .into())
    }
}
