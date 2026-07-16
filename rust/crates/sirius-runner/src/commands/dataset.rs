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
        #[arg(short, long, value_name = "DIR")]
        output: Option<PathBuf>,
        /// Show what would be generated (location, estimated size, free
        /// space) without generating.
        #[arg(short = 'n', long)]
        dry_run: bool,
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
            Self::Generate { .. } => Err(Unimplemented::new(
                "dataset generate",
                "Generate a dataset instance at its keyed location
<data-root>/<family>/sf<N>/<format>[-<compression>][-<encoding>]/: estimate the
size from family and scale factor, check free space on the data root's
filesystem (refusing with a clear message if it won't fit), generate — parquet
via the tpchgen-rs crates in-process with writer-level compression/encoding
control (the sweep dimensions), .duckdb via DuckDB's dbgen so the file format
matches what Sirius reads — and move the result into place atomically.
`bench run` calls this automatically for missing instances.",
            )
            .into()),
            Self::Instances => Err(Unimplemented::new(
                "dataset instances",
                "Scan the data root and list the dataset instances present, with their keyed
parameters (family, scale factor, format, compression, encoding), on-disk size,
and completeness — the inventory `bench run` resolves against and
`dataset generate` fills.",
            )
            .into()),
        }
    }
}
