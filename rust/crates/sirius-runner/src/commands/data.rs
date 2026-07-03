use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented, suite::DataFormat};

/// Generate and inspect benchmark datasets under the data root.
#[derive(Subcommand)]
pub enum Data {
    /// Generate a dataset (disk-aware: checks free space first).
    Generate {
        #[arg(long)]
        benchmark: String,
        #[arg(long)]
        scale_factor: f64,
        #[arg(long, value_enum)]
        format: DataFormat,
        #[arg(long)]
        compression: Option<String>,
        /// Write here instead of the keyed location under the data root.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// List datasets present under the data root.
    List {
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

impl Data {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Generate { .. } => Unimplemented("data generate"),
            Self::List { .. } => Unimplemented("data list"),
        }
        .into())
    }
}
