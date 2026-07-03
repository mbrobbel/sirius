use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Inspect, export, and publish stored benchmark results.
#[derive(Subcommand)]
pub enum Results {
    /// Print the results-store schema (DDL).
    Schema,
    /// List stored runs.
    List,
    /// Show one run.
    Show { run_id: String },
    /// Export a run to files (e.g. for nightly benchmark output).
    Export {
        run_id: String,
        #[arg(long, default_value = "csv")]
        format: String,
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Post a run to the remote results database.
    Push {
        run_id: String,
        #[arg(long)]
        endpoint: Option<String>,
    },
}

impl Results {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Schema => Unimplemented("results schema"),
            Self::List => Unimplemented("results list"),
            Self::Show { .. } => Unimplemented("results show"),
            Self::Export { .. } => Unimplemented("results export"),
            Self::Push { .. } => Unimplemented("results push"),
        }
        .into())
    }
}
