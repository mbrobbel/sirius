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
            Self::Run { .. } => Unimplemented("sweep run"),
        }
        .into())
    }
}
