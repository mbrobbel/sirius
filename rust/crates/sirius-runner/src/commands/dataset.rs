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
    Show {
        /// Dataset family name, e.g. tpch.
        name: String,
    },
    /// Generate a dataset instance (disk-aware: checks free space first).
    Generate {
        /// Dataset family, e.g. tpch.
        name: String,
        /// Scale factor of the instance, e.g. 1, 100.
        #[arg(long)]
        scale_factor: f64,
        /// Storage format.
        #[arg(long, value_enum)]
        format: DataFormat,
        /// Parquet compression codec, e.g. snappy, zstd, uncompressed.
        #[arg(long)]
        compression: Option<String>,
        /// Parquet column encoding, e.g. plain, dictionary.
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
