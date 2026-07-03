use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;
mod stub;
mod suite;

fn main() -> ExitCode {
    match cli::Cli::parse().run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
