//! Standalone Sirius DataFusion CLI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use datafusion::prelude::{ParquetReadOptions, SessionContext};
use sirius_datafusion::SiriusDataFusion;

#[derive(Debug, Parser)]
#[command(about = "Plan a DataFusion SQL query and execute it with Sirius")]
struct Args {
    /// SQL text to execute.
    #[arg(long, conflicts_with = "query_file")]
    sql: Option<String>,

    /// File containing the SQL query to execute.
    #[arg(long, value_name = "PATH", conflicts_with = "sql")]
    query_file: Option<PathBuf>,

    /// Register a Parquet table as NAME=PATH. May be repeated.
    #[arg(long = "table", value_name = "NAME=PATH")]
    tables: Vec<String>,

    /// Sirius YAML configuration file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let sql = match (args.sql, args.query_file) {
        (Some(sql), None) => sql,
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("read query file {}", path.display()))?,
        (None, None) => bail!("provide either --sql or --query-file"),
        (Some(_), Some(_)) => unreachable!("clap enforces argument conflicts"),
    };

    let ctx = SessionContext::new();
    for table in args.tables {
        let (name, path) = table
            .split_once('=')
            .with_context(|| format!("invalid --table {table:?}; expected NAME=PATH"))?;
        ctx.register_parquet(name, path, ParquetReadOptions::default())
            .await
            .with_context(|| format!("register Parquet table {name} from {path}"))?;
    }

    let sirius = match args.config {
        Some(path) => SiriusDataFusion::from_config_file(path)?,
        None => SiriusDataFusion::new()?,
    };
    let batches = sirius.execute_query(&ctx, &sql).await?;
    let rows: usize = batches.iter().map(|batch| batch.num_rows()).sum();
    println!("{rows} rows in {} record batches", batches.len());
    Ok(())
}
