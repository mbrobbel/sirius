mod config;
mod corpus;
mod name;
mod report;
mod result;
mod run;
mod snapshot;
mod substitutions;
mod worker;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sirius-sqltest",
    about = "Artifact-based SQL correctness testing for Sirius"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run(run::RunArgs),
    Complete(snapshot::CompleteArgs),
    Report {
        input: PathBuf,
        #[arg(long)]
        previous: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    #[command(hide = true)]
    Worker {
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        extension: Option<PathBuf>,
    },
}

fn main() {
    match execute() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(2);
        }
    }
}

fn execute() -> Result<bool> {
    match Cli::parse().command {
        Commands::Run(args) => return run::execute(&args),
        Commands::Complete(args) => snapshot::complete(&args)?,
        Commands::Worker { socket, extension } => worker::run(&socket, extension.as_deref())?,
        Commands::Report {
            input,
            previous,
            output,
        } => {
            let report: report::Report = serde_json::from_slice(&std::fs::read(input)?)?;
            let previous = previous
                .map(|p| -> Result<report::Report> {
                    Ok(serde_json::from_slice(&std::fs::read(p)?)?)
                })
                .transpose()?;
            if let Some(output) = output {
                report.save(&output, previous.as_ref())?;
            } else {
                print!("{}", report.markdown(previous.as_ref()));
            }
        }
    }
    Ok(true)
}
