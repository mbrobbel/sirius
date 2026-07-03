use std::path::PathBuf;

use clap::Subcommand;

use crate::{
    cli::GlobalArgs,
    stub::Unimplemented,
    suite::{RunMode, Suites},
};

/// List, inspect, and run benchmark suites.
#[derive(Subcommand)]
pub enum Suite {
    /// List available suites (embedded, or --suites directory).
    List,
    /// Show a suite's manifest.
    Show { name: String },
    /// Run a suite: resolve (or generate) its dataset, run its queries.
    Run {
        name: String,
        /// Subset of queries to run, e.g. q1,q6.
        #[arg(long, value_delimiter = ',')]
        queries: Vec<String>,
        /// Build preset to run against.
        #[arg(long)]
        preset: Option<String>,
        /// Use this dataset directory instead of resolving the suite's spec.
        #[arg(long, value_name = "DIR")]
        data_dir: Option<PathBuf>,
        /// Fail if the dataset is missing instead of generating it.
        #[arg(long)]
        no_generate: bool,
        #[arg(long)]
        iterations: Option<u32>,
        #[arg(long, value_enum)]
        mode: Option<RunMode>,
    },
}

impl Suite {
    pub fn run(&self, globals: &GlobalArgs) -> anyhow::Result<()> {
        let suites = Suites::resolve(globals.suites.as_deref());
        match self {
            Self::List => {
                for name in suites.names()? {
                    println!("{name}");
                }
                Ok(())
            }
            Self::Show { name } => {
                let manifest = suites.load(name)?;
                if globals.json {
                    println!("{}", serde_json::to_string_pretty(&manifest)?);
                } else {
                    print!("{}", toml::to_string_pretty(&manifest)?);
                }
                Ok(())
            }
            Self::Run { .. } => Err(Unimplemented("suite run").into()),
        }
    }
}
