use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Manage remote machines.
///
/// Execution on a remote goes through the global --remote flag; this group
/// only manages targets.
#[derive(Subcommand)]
pub enum Remote {
    /// Register a named remote.
    Add {
        name: String,
        #[arg(long)]
        host: String,
    },
    /// List registered remotes.
    List,
    /// Report a remote's bootstrap status (env pack, runner binary, build).
    Status { name: String },
    /// Pre-build the pixi-pack environment archive for a target platform.
    Pack {
        #[arg(long, value_name = "PIXI_ENV")]
        env: Option<String>,
        #[arg(long)]
        platform: Option<String>,
    },
}

impl Remote {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Add { .. } => Unimplemented("remote add"),
            Self::List => Unimplemented("remote list"),
            Self::Status { .. } => Unimplemented("remote status"),
            Self::Pack { .. } => Unimplemented("remote pack"),
        }
        .into())
    }
}
