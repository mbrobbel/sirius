use std::path::PathBuf;

use clap::Subcommand;

use crate::{
    assets::{Assets, Engine, RunMode},
    cli::GlobalArgs,
    stub::Unimplemented,
};

/// List, inspect, and run benchmarks (run configurations).
#[derive(Subcommand)]
pub enum Bench {
    /// List available benchmarks (embedded, or --assets directory).
    List,
    /// Show a benchmark's manifest.
    Show {
        /// Benchmark name, e.g. tpch-sf1.
        name: String,
    },
    /// Run a benchmark: resolve its dataset instance and expected results
    /// (generating what's missing), check the Sirius build and engine
    /// config, run the suite, validate and store results.
    Run {
        /// Benchmark name; or use --sql/--query for an ad-hoc run.
        name: Option<String>,
        /// Ad-hoc: SQL file to run.
        #[arg(long, conflicts_with = "name", value_name = "FILE")]
        sql: Option<PathBuf>,
        /// Ad-hoc: named query from a suite, e.g. tpch/q6.
        #[arg(long, conflicts_with_all = ["name", "sql"])]
        query: Option<String>,
        /// Ad-hoc: dataset instance as <family>:<scale-factor>:<format>.
        #[arg(long, conflicts_with = "name")]
        dataset: Option<String>,
        /// Subset of queries to run, e.g. q1,q6.
        #[arg(long, value_delimiter = ',')]
        queries: Vec<String>,
        /// Build preset to run against.
        #[arg(long)]
        preset: Option<String>,
        /// Use this dataset directory instead of resolving instance args.
        #[arg(long, value_name = "DIR")]
        data_dir: Option<PathBuf>,
        /// Fail if the dataset instance is missing instead of generating it.
        #[arg(long)]
        no_generate: bool,
        /// Iterations per query; overrides the manifest.
        #[arg(long)]
        iterations: Option<u32>,
        /// Cold (fresh process, dropped caches) or warm execution; overrides
        /// the manifest.
        #[arg(long, value_enum)]
        mode: Option<RunMode>,
        /// Engine(s) to run; overrides the manifest.
        #[arg(long, value_enum)]
        engine: Option<Engine>,
        /// Sirius engine config (YAML); overrides the manifest.
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

impl Bench {
    pub fn run(&self, globals: &GlobalArgs) -> anyhow::Result<()> {
        let assets = Assets::resolve(globals.assets.as_deref());
        match self {
            Self::List => globals.print_names(assets.bench_names()?),
            Self::Show { name } => globals.print_manifest(&assets.load_bench(name)?),
            Self::Run { .. } => Err(Unimplemented("bench run").into()),
        }
    }
}
