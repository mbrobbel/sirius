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
            Self::List => Unimplemented::new(
                "results list",
                "List the runs in the local results store (a DuckDB file): id, benchmark,
dataset instance, engine(s), environment, when it ran, a timing summary, and
validation status. The store is the single source of truth — runs executed
with --remote land here too.",
            ),
            Self::Show { .. } => Unimplemented::new(
                "results show",
                "Show one stored run in full: its manifest and config snapshot, the environment
it ran on, per-query and per-iteration runtimes, errors/timeouts, and
validation outcomes.",
            ),
            Self::Export { .. } => Unimplemented::new(
                "results export",
                "Write a stored run to files (CSV or JSON): runtimes, validations, and
metadata. The output-file path for nightly benchmark artifacts and any
external tooling that doesn't read the store directly.",
            ),
            Self::Push { .. } => Unimplemented::new(
                "results push",
                "Publish a stored run — with its environment and per-query results, and the
engine-config snapshot redacted of secrets — to the remote results database at
accel-etl.nvidia.com, for fleet-wide dashboards and long-term regression
tracking across machines and versions.",
            ),
        }
        .into())
    }
}
