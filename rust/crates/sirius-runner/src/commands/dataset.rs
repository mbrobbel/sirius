use std::path::PathBuf;

use clap::Subcommand;

use crate::{
    assets::{Assets, DataFormat},
    cli::GlobalArgs,
    stub::Unimplemented,
};

/// List dataset families and generate instances under the data root.
#[derive(Subcommand)]
pub enum Dataset {
    /// List dataset families (embedded, or --assets directory).
    List,
    /// Show a dataset family's manifest.
    Show { name: String },
    /// Generate a dataset instance (disk-aware: checks free space first).
    Generate {
        /// Dataset family, e.g. tpch.
        name: String,
        #[arg(long)]
        scale_factor: f64,
        #[arg(long, value_enum)]
        format: DataFormat,
        #[arg(long)]
        compression: Option<String>,
        #[arg(long)]
        encoding: Option<String>,
        /// Write here instead of the keyed location under the data root.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// List instances present under the data root.
    Instances,
}

impl Dataset {
    pub fn run(&self, globals: &GlobalArgs) -> anyhow::Result<()> {
        let assets = Assets::resolve(globals.assets.as_deref());
        match self {
            Self::List => globals.print_names(assets.dataset_names()?),
            Self::Show { name } => globals.print_manifest(&assets.load_dataset(name)?),
            Self::Generate { .. } => Err(Unimplemented("dataset generate").into()),
            Self::Instances => Err(Unimplemented("dataset instances").into()),
        }
    }
}
