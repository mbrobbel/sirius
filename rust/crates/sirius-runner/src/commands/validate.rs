use std::path::PathBuf;

use clap::Subcommand;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Generate and check expected results for correctness validation.
///
/// Expected results are keyed by the logical dataset instance
/// (expected/<suite>/sf<N>/) — independent of format/compression/encoding.
#[derive(Subcommand)]
pub enum Validate {
    /// Generate expected results for a suite at a scale factor using the
    /// suite's reference engine, and cache them.
    Generate {
        /// Query suite name, e.g. tpch.
        suite: String,
        /// Scale factor to generate expected results for.
        #[arg(long)]
        scale_factor: f64,
        /// Overrides the suite's reference engine.
        #[arg(long)]
        engine: Option<String>,
        /// Write here instead of the keyed location.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
    /// Report which expected results are present for a suite.
    Status {
        /// Query suite name, e.g. tpch.
        suite: String,
        /// Only report this scale factor.
        #[arg(long)]
        scale_factor: Option<f64>,
    },
    /// Compare a stored run's results against expected results.
    Compare {
        /// Stored run to check.
        #[arg(long)]
        run: String,
    },
}

impl Validate {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(match self {
            Self::Generate { .. } => Unimplemented("validate generate"),
            Self::Status { .. } => Unimplemented("validate status"),
            Self::Compare { .. } => Unimplemented("validate compare"),
        }
        .into())
    }
}
