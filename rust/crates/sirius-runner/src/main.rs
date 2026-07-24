use std::{
    io::{self, Write},
    process::ExitCode,
};

use clap::{Parser, error::ErrorKind};
use sirius_runner::{
    cancel,
    cli::{Cli, is_broken_pipe},
};

fn main() -> ExitCode {
    if let Err(error) = cancel::install() {
        let _ = writeln!(
            io::stderr().lock(),
            "error: installing signal handlers: {error:#}"
        );
        return ExitCode::FAILURE;
    }
    let json_requested = std::env::args_os().any(|argument| argument == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            if json_requested {
                let document = serde_json::json!({
                    "status": "usage_error",
                    "message": error.to_string(),
                    "exit_code": 2,
                });
                if writeln!(io::stdout().lock(), "{document}")
                    .is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
                {
                    return ExitCode::SUCCESS;
                }
            } else {
                let _ = error.print();
            }
            return ExitCode::from(2);
        }
    };
    let json = cli.globals.json;
    match cli.run() {
        Ok(outcome) => ExitCode::from(outcome.exit_code()),
        Err(error) if is_broken_pipe(&error) => ExitCode::SUCCESS,
        Err(error) if cancel::is_interrupted(&error) => {
            if json {
                let document = serde_json::json!({
                    "status": "interrupted",
                    "message": format!("{error:#}"),
                });
                let _ = writeln!(io::stdout().lock(), "{document}");
            } else {
                let _ = writeln!(io::stderr().lock(), "interrupted: {error:#}");
            }
            ExitCode::from(130)
        }
        Err(error) => {
            if json {
                let document = serde_json::json!({
                    "status": "error",
                    "message": format!("{error:#}"),
                });
                if writeln!(io::stdout().lock(), "{document}")
                    .is_err_and(|error| error.kind() == io::ErrorKind::BrokenPipe)
                {
                    return ExitCode::SUCCESS;
                }
            } else {
                let _ = writeln!(io::stderr().lock(), "error: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}
