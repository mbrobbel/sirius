use std::process::ExitCode;

use clap::Parser;

mod assets;
mod cli;
mod commands;
mod store;
mod stub;

fn main() -> ExitCode {
    match cli::Cli::parse().run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}
