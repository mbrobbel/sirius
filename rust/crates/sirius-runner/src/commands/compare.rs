use clap::Args;

use crate::{cli::GlobalArgs, stub::Unimplemented};

/// Compare two stored runs.
#[derive(Args)]
pub struct Compare {
    /// Baseline run.
    pub run_a: String,
    /// Run to compare against the baseline.
    pub run_b: String,
}

impl Compare {
    pub fn run(&self, _globals: &GlobalArgs) -> anyhow::Result<()> {
        Err(Unimplemented::new(
            "compare",
            "Compare two stored runs query by query: runtime ratios and speedups,
regressions beyond a threshold, and validation-status changes — e.g. a PR
build against dev, cold vs warm, or the same benchmark on two machines or
engine configs. Runs entirely on the local store (SQL over the DuckDB file);
--json for CI gating.",
        )
        .into())
    }
}
