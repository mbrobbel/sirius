use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Generate and check expected results for correctness validation.
#[derive(Subcommand)]
pub enum Validate {
    /// Generate expected results for a suite from a reference engine.
    Generate {
        suite: String,
        #[arg(long, default_value = "duckdb")]
        engine: String,
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Compare a run's results against the suite's expected results.
    Compare {
        #[arg(long)]
        run: String,
    },
}

impl Validate {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Generate { .. } => Unimplemented("validate generate"),
            Self::Compare { .. } => Unimplemented("validate compare"),
        }
        .into())
    }
}
