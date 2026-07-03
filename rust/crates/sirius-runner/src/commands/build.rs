use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Make Sirius available: discover, build, or download it.
#[derive(Subcommand)]
pub enum Build {
    /// Discover existing builds under build/<preset>/extension/sirius
    /// (honors SIRIUS_BUILD_DIR).
    List,
    /// Build from source via `pixi run make <preset>`.
    Source {
        #[arg(long, default_value = "release")]
        preset: String,
    },
    /// Download a recent build artifact from GitHub.
    Download {
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        run_id: Option<u64>,
    },
}

impl Build {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::List => Unimplemented("build list"),
            Self::Source { .. } => Unimplemented("build source"),
            Self::Download { .. } => Unimplemented("build download"),
        }
        .into())
    }
}
