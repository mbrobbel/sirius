mod database;
mod standard;

use anyhow::{Result, ensure};
use clap::Parser;
use database::Database;
use duckdb::Connection;
use sqllogictest::{Runner, strict_column_validator};
use standard::Script;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(about = "Check standard SQLLogicTest files against DuckDB")]
struct Args {
    /// SQLLogicTest files to run; expand globs in your shell.
    #[arg(required = true, num_args = 1..)]
    files: Vec<PathBuf>,
}

fn run_file(path: &Path) -> Result<()> {
    let script = Script::read(path)?;
    let directory = tempfile::tempdir()?;
    let connection = Connection::open(directory.path().join("test.duckdb"))?;
    let mut runner =
        Runner::new(|| async { Ok::<_, database::Error>(Database::new(connection.try_clone()?)) });
    runner.with_column_validator(strict_column_validator);
    runner.with_validator(standard::compare);
    runner.run_multi(script.records())?;
    runner.shutdown();
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut failed = 0;
    for file in &args.files {
        match run_file(file) {
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
