use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::commands;

/// Benchmark runner for Sirius: builds, datasets, suites, benchmarks, results.
#[derive(Parser)]
#[command(version, propagate_version = true)]
#[command(after_help = "\
Examples:
  sirius-runner bench run tpch-sf1        run a benchmark end to end
  sirius-runner bench list                see what can be run
  sirius-runner suite show tpch           inspect a query suite
  sirius-runner specs                     report this machine's hardware

Docs & issues: https://github.com/sirius-db/sirius (rust/crates/sirius-runner)")]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    pub fn run(self) -> anyhow::Result<()> {
        let globals = &self.globals;
        match self.command {
            Command::Specs(cmd) => cmd.run(globals),
            Command::Build(cmd) => cmd.run(globals),
            Command::Dataset(cmd) => cmd.run(globals),
            Command::Suite(cmd) => cmd.run(globals),
            Command::Bench(cmd) => cmd.run(globals),
            Command::Validate(cmd) => cmd.run(globals),
            Command::Results(cmd) => cmd.run(globals),
            Command::Compare(cmd) => cmd.run(globals),
            Command::Sweep(cmd) => cmd.run(globals),
            Command::Telemetry(cmd) => cmd.run(globals),
            Command::Remote(cmd) => cmd.run(globals),
        }
    }
}

#[derive(Args)]
#[command(next_help_heading = "Global options")]
pub struct GlobalArgs {
    /// Sirius checkout root; defaults to walking up from the current directory
    /// to the first directory containing pixi.toml.
    #[arg(long, global = true, env = "SIRIUS_REPO_ROOT", value_name = "DIR")]
    pub repo_root: Option<PathBuf>,

    /// Load benchmark definitions from this directory instead of the embedded
    /// set (same layout: datasets/, suites/, benches/, expected/).
    #[arg(long, global = true, env = "SIRIUS_RUNNER_ASSETS", value_name = "DIR")]
    pub assets: Option<PathBuf>,

    /// Root directory for generated and discovered dataset instances.
    #[arg(
        long,
        global = true,
        env = "SIRIUS_RUNNER_DATA_ROOT",
        value_name = "DIR",
        default_value = "test_datasets"
    )]
    pub data_root: PathBuf,

    /// Execute this invocation on a remote machine (named remote or user@host).
    #[arg(long, global = true, env = "SIRIUS_RUNNER_REMOTE", value_name = "HOST")]
    pub remote: Option<String>,

    /// Machine-readable JSON output where a command supports it.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress non-essential output (progress, hints).
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Never prompt; fail instead of asking for confirmation (for scripts/CI).
    #[arg(long, global = true)]
    pub no_input: bool,

    /// Disable colored output (also honored: NO_COLOR, TERM=dumb, non-TTY).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Increase output verbosity.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

impl GlobalArgs {
    pub fn print_names(&self, names: Vec<String>) -> anyhow::Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(&names)?);
        } else {
            for name in names {
                println!("{name}");
            }
        }
        Ok(())
    }

    pub fn print_manifest<T: Serialize>(&self, manifest: &T) -> anyhow::Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(manifest)?);
        } else {
            print!("{}", toml::to_string_pretty(manifest)?);
        }
        Ok(())
    }
}

#[derive(Subcommand)]
pub enum Command {
    Specs(commands::specs::Specs),
    #[command(subcommand)]
    Build(commands::build::Build),
    #[command(subcommand)]
    Dataset(commands::dataset::Dataset),
    #[command(subcommand)]
    Suite(commands::suite::Suite),
    #[command(subcommand)]
    Bench(commands::bench::Bench),
    #[command(subcommand)]
    Validate(commands::validate::Validate),
    #[command(subcommand)]
    Results(commands::results::Results),
    Compare(commands::compare::Compare),
    #[command(subcommand)]
    Sweep(commands::sweep::Sweep),
    #[command(subcommand)]
    Telemetry(commands::telemetry::Telemetry),
    #[command(subcommand)]
    Remote(commands::remote::Remote),
}
