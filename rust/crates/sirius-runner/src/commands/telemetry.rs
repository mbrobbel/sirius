use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// View quent telemetry from benchmark runs.
#[derive(Subcommand)]
pub enum Telemetry {
    /// Serve the telemetry UI over an output directory.
    Serve {
        #[arg(long, value_name = "DIR", default_value = "telemetry_data")]
        output_dir: PathBuf,
    },
    /// Summarize telemetry for a run.
    View {
        #[arg(long)]
        run: String,
    },
}

impl Telemetry {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Serve { .. } => Unimplemented("telemetry serve"),
            Self::View { .. } => Unimplemented("telemetry view"),
        }
        .into())
    }
}
