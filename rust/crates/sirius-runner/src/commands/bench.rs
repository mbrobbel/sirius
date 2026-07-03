use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented, suite::RunMode};

/// Run ad-hoc benchmarks outside a suite.
#[derive(Subcommand)]
pub enum Bench {
    /// Run a single query against a dataset.
    #[command(group = clap::ArgGroup::new("source").required(true))]
    Run {
        /// SQL file to run.
        #[arg(long, group = "source")]
        sql: Option<PathBuf>,
        /// Named query from an embedded suite, e.g. tpch/q6.
        #[arg(long, group = "source")]
        query: Option<String>,
        /// Dataset spec as <benchmark>:<scale-factor>:<format>, e.g. tpch:1:parquet.
        #[arg(long)]
        dataset: String,
        #[arg(long, default_value_t = 1)]
        iterations: u32,
        #[arg(long, value_enum, default_value_t = RunMode::Warm)]
        mode: RunMode,
        /// Sirius engine config (YAML).
        #[arg(long, value_name = "FILE")]
        config: Option<PathBuf>,
    },
}

impl Bench {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Run { .. } => Unimplemented("bench run"),
        }
        .into())
    }
}
