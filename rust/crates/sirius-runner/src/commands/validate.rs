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
            Self::Generate { .. } => Unimplemented::new(
                "validate generate",
                "Run the suite's queries on its reference engine (plain DuckDB on CPU) against
the logical dataset at the given scale factor, and write expected results to
expected/<suite>/sf<N>/ (or --out): full rows for tolerance-compared queries,
digests for queries whose result size scales with the dataset. This is slow at
large scale factors by nature — which is exactly why generated sets are
committed alongside the suites (and cached under the data root), so CI and
developers only ever compare, never regenerate.",
            ),
            Self::Status { .. } => Unimplemented::new(
                "validate status",
                "Report which expected results exist for the suite — per scale factor and per
query, and from which source (assets directory or data-root cache) — and which
are missing and would need `validate generate`. `bench run` performs the same
resolution before a validating run.",
            ),
            Self::Compare { .. } => Unimplemented::new(
                "validate compare",
                "Re-check a stored run against expected results: per query, compare the run's
stored result evidence using the suite's strategy (tolerance-aware row
comparison or digest) and record match/mismatch/error rows in the validations
table. Normally `bench run` validates inline; this recovers or re-judges after
the fact, e.g. with regenerated expected data.",
            ),
        }
        .into())
    }
}
