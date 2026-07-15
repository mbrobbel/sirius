use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// View quent telemetry from benchmark runs.
#[derive(Subcommand)]
pub enum Telemetry {
    /// Serve the telemetry UI over an output directory.
    Serve {
        /// Quent telemetry output directory to serve.
        #[arg(long, value_name = "DIR", default_value = "telemetry_data")]
        output_dir: PathBuf,
    },
    /// Summarize telemetry for a run.
    View {
        /// Stored run whose telemetry to view.
        #[arg(long)]
        run: String,
    },
}

impl Telemetry {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Serve { .. } => Unimplemented::new(
                "telemetry serve",
                "Launch the Quent telemetry UI (the sirius-telemetry-server crate — collector
plus browser analyzer, what `pixi run quent` does today) over the given output
directory, so runs executed with telemetry enabled in the engine config can be
explored interactively.",
            ),
            Self::View { .. } => Unimplemented::new(
                "telemetry view",
                "Summarize a stored run's Quent telemetry in the terminal: per-operator and
per-pipeline time, memory pressure and downgrade events — the quick look
before reaching for the full UI. The run's telemetry_path column points at
the data, also for runs pulled back from a remote.",
            ),
        }
        .into())
    }
}
