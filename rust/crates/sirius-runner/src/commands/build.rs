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
        /// CMake preset to build, e.g. release, debug, relwithdebinfo.
        #[arg(long, default_value = "release")]
        preset: String,
    },
    /// Download a recent CI build artifact from GitHub.
    Download {
        /// Latest successful build from this branch (default: dev).
        #[arg(long)]
        branch: Option<String>,
        /// Commit whose CI build to download (resolved to a workflow run).
        #[arg(long, conflicts_with_all = ["branch", "run_id"])]
        commit: Option<String>,
        /// Exact GitHub Actions workflow run to download artifacts from.
        #[arg(long)]
        run_id: Option<u64>,
    },
}

impl Build {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::List => Unimplemented::new(
                "build list",
                "Discover usable Sirius builds: scan build/<preset>/extension/sirius/ for the
extension and the DuckDB binary (honoring SIRIUS_BUILD_DIR), and report each
build's preset, commit, and age relative to the working tree. This is the
inventory `bench run --preset` selects from.",
            ),
            Self::Source { .. } => Unimplemented::new(
                "build source",
                "Build Sirius from source by invoking `pixi run make <preset>` at the repo root
and verifying the extension and DuckDB binary appear under build/<preset>/.
`bench run` falls back to this when no usable build exists.",
            ),
            Self::Download { .. } => Unimplemented::new(
                "build download",
                "Fetch a CI-built extension without a local toolchain: resolve
--branch/--commit/--run-id to a GitHub Actions workflow run, download its build
artifact (build-cuda-*), and unpack it into the build tree so `bench run` uses
the exact binaries CI tested. Also how remote machines get their build staged.",
            ),
        }
        .into())
    }
}
