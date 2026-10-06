mod database;
mod standard;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use database::Database;
use duckdb::{Config, Connection};
use sqllogictest::{Runner, strict_column_validator};
use standard::Script;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about = "Check standard SQLLogicTest files against DuckDB or Sirius")]
struct Args {
    /// Load this Sirius extension; otherwise use plain DuckDB.
    #[arg(long)]
    extension: Option<PathBuf>,

    /// SQLLogicTest files to run; expand globs in your shell.
    #[arg(required = true, num_args = 1..)]
    files: Vec<PathBuf>,
}

fn run_file(path: &Path, extension: Option<&Path>) -> Result<()> {
    let script = Script::read(path)?;
    let directory = tempfile::tempdir()?;
    let config = if extension.is_some() {
        Config::default().allow_unsigned_extensions()?
    } else {
        Config::default()
    };
    let connection = Connection::open_with_flags(directory.path().join("test.duckdb"), config)?;
    if let Some(extension) = extension {
        ensure!(
            std::env::var_os("SIRIUS_DISABLE").is_none_or(|value| value == "0"),
            "--extension requires Sirius execution: unset SIRIUS_DISABLE or set it to 0"
        );
        let extension = extension.to_str().context("extension path must be UTF-8")?;
        connection.execute_batch(&format!("LOAD '{}';", extension.replace('\'', "''")))?;
        connection
            .execute_batch("SET enable_duckdb_fallback = false; SET gpu_execution = true;")?;
    }
    // Initialize before the runner so setup failures cannot satisfy statement error.
    let mut database = Some(Database::new(connection, extension.is_some()));
    let mut runner = Runner::new(|| {
        std::future::ready(
            database
                .take()
                .ok_or(database::Error::ConnectionAlreadyUsed),
        )
    });
    runner.with_column_validator(strict_column_validator);
    runner.with_validator(standard::compare);
    runner.run_multi(script.records())?;
    runner.shutdown();
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let extension = args
        .extension
        .as_ref()
        .map(|path| {
            path.canonicalize()
                .with_context(|| format!("extension {}", path.display()))
        })
        .transpose()?;
    let mut failed = 0;
    for file in &args.files {
        match run_file(file, extension.as_deref()) {
            Ok(()) => println!("PASS {}", file.display()),
            Err(error) => {
                failed += 1;
                eprintln!("FAIL {}\n{error:#}", file.display());
            }
        }
    }
    println!("{} passed; {failed} failed", args.files.len() - failed);
    ensure!(failed == 0, "SQLLogicTest failures");
    Ok(())
}
