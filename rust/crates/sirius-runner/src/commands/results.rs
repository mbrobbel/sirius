use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, store::Store, stub::Unimplemented};

/// Inspect, export, and publish stored benchmark results.
#[derive(Subcommand)]
pub enum Results {
    /// Print the results-store schema (DDL).
    Schema,
    /// List stored runs.
    List,
    /// Show one run.
    Show {
        /// Stored run to show.
        run_id: String,
    },
    /// Export a run to files (e.g. for nightly benchmark output).
    Export {
        /// Stored run to export.
        run_id: String,
        /// Output format: csv or json.
        #[arg(long, default_value = "csv")]
        format: String,
        /// Write files here instead of the current directory.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Post a run to the remote results database.
    Push {
        /// Stored run to publish.
        run_id: String,
        /// Results database endpoint (default: accel-etl.nvidia.com).
        #[arg(long)]
        endpoint: Option<String>,
    },
}

impl Results {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Schema => {
                print!("{}", Store::SCHEMA);
                return Ok(());
            }
            Self::List => Unimplemented("results list"),
            Self::Show { .. } => Unimplemented("results show"),
            Self::Export { .. } => Unimplemented("results export"),
            Self::Push { .. } => Unimplemented("results push"),
        }
        .into())
    }
}
