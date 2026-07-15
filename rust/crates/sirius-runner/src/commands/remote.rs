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
        /// Name to register the remote under.
        name: String,
        /// SSH destination (user@host).
        #[arg(long)]
        host: String,
    },
    /// List registered remotes.
    List,
    /// Report a remote's bootstrap status (env pack, runner binary, build).
    Status {
        /// Registered remote name.
        name: String,
    },
    /// Pre-build the pixi-pack environment archive for a target platform.
    Pack {
        /// Pixi environment to pack (default: the runtime environment).
        #[arg(long, value_name = "PIXI_ENV")]
        env: Option<String>,
        /// Target platform, e.g. linux-64, linux-aarch64.
        #[arg(long)]
        platform: Option<String>,
    },
}

impl Remote {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Add { .. } => Unimplemented::new(
                "remote add",
                "Register a named remote (an SSH destination) in the runner's config and verify
connectivity, so `--remote <name>` works on any execution command instead of
spelling out user@host each time.",
            ),
            Self::List => Unimplemented::new(
                "remote list",
                "List registered remotes with their host, platform/architecture, and last-known
bootstrap state.",
            ),
            Self::Status { .. } => Unimplemented::new(
                "remote status",
                "Probe a remote over SSH: reachability, architecture, GPU and driver, and which
components are already staged there — env pack, runner binary, Sirius build,
dataset instances. What --remote checks (and fixes) implicitly before running.",
            ),
            Self::Pack { .. } => Unimplemented::new(
                "remote pack",
                "Build the pixi-pack archive of the runtime environment for a target platform
from pixi.lock — the bundle --remote ships to bootstrap a machine that has no
pixi or conda (unpacked there via pixi-unpack into ./env + activate.sh). Run it
ahead of time, e.g. in CI, to make first use of a new remote fast; cross-
platform packing works because packages come from the lockfile.",
            ),
        }
        .into())
    }
}
