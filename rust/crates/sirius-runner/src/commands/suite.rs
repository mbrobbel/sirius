use clap::Subcommand;

use crate::{assets::Assets, cli::GlobalArgs};

/// List and inspect query suites.
#[derive(Subcommand)]
pub enum Suite {
    /// List available suites (embedded, or --assets directory).
    List,
    /// Show a suite's manifest.
    Show {
        /// Query suite name, e.g. tpch.
        name: String,
    },
}

impl Suite {
    pub fn run(&self, globals: &GlobalArgs) -> anyhow::Result<()> {
        let assets = Assets::resolve(globals.assets.as_deref());
        match self {
            Self::List => globals.print_names(assets.suite_names()?),
            Self::Show { name } => globals.print_manifest(&assets.load_suite(name)?),
        }
    }
}
