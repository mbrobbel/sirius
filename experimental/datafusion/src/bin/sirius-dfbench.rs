//! Sirius runner for the libcudf DataFusion benchmark harness.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use datafusion::prelude::{ParquetReadOptions, SessionConfig, SessionContext};
use serde::Serialize;
use sirius_datafusion::SiriusDataFusion;

#[derive(Debug, Parser)]
#[command(about = "Run libcudf DataFusion benchmark queries with Sirius")]
struct Args {
    /// Dataset identifier stored in benchmark result JSON.
    #[arg(long)]
    dataset_name: String,

    /// Directory containing one Parquet directory per table.
    #[arg(long, value_name = "PATH")]
    dataset_path: PathBuf,

    /// Directory containing q*.sql benchmark queries.
    #[arg(long, value_name = "PATH")]
    query_dir: PathBuf,

    /// Query identifiers. If omitted, runs every SQL file in the query directory.
    #[arg(short, long, value_delimiter = ',')]
    query: Vec<String>,

    /// Number of timed iterations per query.
    #[arg(short = 'i', long, default_value_t = 3)]
    iterations: usize,

    /// Number of DataFusion planning partitions.
    #[arg(short = 'n', long)]
    partitions: Option<usize>,

    /// DataFusion batch size used while planning table scans.
    #[arg(short = 's', long)]
    batch_size: Option<usize>,

    /// Run each query once before timed iterations.
    #[arg(long)]
    warmup: bool,

    /// Directory for per-query benchmark JSON files.
    #[arg(long, value_name = "PATH")]
    result_dir: PathBuf,
}

#[derive(Debug, Serialize)]
struct QueryIteration {
    row_count: usize,
    elapsed: f64,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BenchmarkResult {
    id: String,
    dataset: String,
    iterations: Vec<QueryIteration>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_dir(&args.dataset_path, "dataset")?;
    validate_dir(&args.query_dir, "query")?;
    fs::create_dir_all(&args.result_dir).with_context(|| {
        format!(
            "create benchmark result directory {}",
            args.result_dir.display()
        )
    })?;

    let mut config = SessionConfig::from_env()?;
    config = config.with_target_partitions(
        args.partitions
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from)),
    );
    if let Some(batch_size) = args.batch_size {
        config = config.with_batch_size(batch_size);
    }
    let ctx = SessionContext::new_with_config(config);
    register_tables(&ctx, &args.dataset_path).await?;

    let query_ids = selected_queries(&args.query_dir, &args.query)?;
    if query_ids.is_empty() {
        bail!("no SQL queries found in {}", args.query_dir.display());
    }

    let sirius = SiriusDataFusion::new()?;
    println!("Running Sirius benchmarks with the following options: {args:?}");

    for query_id in query_ids {
        let sql_path = args.query_dir.join(format!("{query_id}.sql"));
        let sql = fs::read_to_string(&sql_path)
            .with_context(|| format!("read benchmark query {}", sql_path.display()))?;
        apply_query_settings(&ctx, &sql).await?;

        let result = benchmark_query(&args, &ctx, &sirius, &query_id, &sql).await;
        let output_path = args.result_dir.join(format!("{query_id}.json"));
        fs::write(&output_path, serde_json::to_string_pretty(&result)?)
            .with_context(|| format!("write benchmark result {}", output_path.display()))?;
    }

    Ok(())
}

async fn benchmark_query(
    args: &Args,
    ctx: &SessionContext,
    sirius: &SiriusDataFusion,
    query_id: &str,
    sql: &str,
) -> BenchmarkResult {
    let id = format!("{} {query_id}", args.dataset_name);

    if args.warmup {
        for statement in statements(sql) {
            let result = execute_statement(ctx, sirius, statement).await;
            match result {
                Ok(_) => println!("Query {id} warmup completed"),
                Err(error) => println!("Query {id} warmup failed: {error}"),
            }
        }
    }

    let mut iterations = Vec::with_capacity(args.iterations);
    'iteration: for iteration in 0..args.iterations {
        let start = Instant::now();
        for statement in statements(sql) {
            if is_setup_statement(statement) {
                if let Err(error) = execute_datafusion(ctx, statement).await {
                    iterations.push(failed_iteration(error));
                    continue 'iteration;
                }
                continue;
            }

            match sirius.execute_query(ctx, statement).await {
                Ok(batches) => {
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    let row_count = batches.iter().map(|batch| batch.num_rows()).sum();
                    println!(
                        "Query {id} iteration {iteration} took {elapsed:.1} ms and returned {row_count} rows"
                    );
                    iterations.push(QueryIteration {
                        row_count,
                        elapsed,
                        error: None,
                    });
                }
                Err(error) => {
                    println!("Query {id} iteration {iteration} failed: {error}");
                    iterations.push(failed_iteration(error));
                    continue 'iteration;
                }
            }
        }
    }

    let successful = iterations
        .iter()
        .filter(|iteration| iteration.error.is_none())
        .collect::<Vec<_>>();
    if !successful.is_empty() {
        let average = successful
            .iter()
            .map(|iteration| iteration.elapsed)
            .sum::<f64>()
            / successful.len() as f64;
        println!("Query {id} avg time: {average:.2} ms");
    }

    BenchmarkResult {
        id,
        dataset: args.dataset_name.clone(),
        iterations,
    }
}

async fn execute_statement(
    ctx: &SessionContext,
    sirius: &SiriusDataFusion,
    sql: &str,
) -> datafusion::error::Result<usize> {
    if is_setup_statement(sql) {
        return execute_datafusion(ctx, sql).await;
    }
    let batches = sirius.execute_query(ctx, sql).await?;
    Ok(batches.iter().map(|batch| batch.num_rows()).sum())
}

async fn execute_datafusion(ctx: &SessionContext, sql: &str) -> datafusion::error::Result<usize> {
    let batches = ctx.sql(sql).await?.collect().await?;
    Ok(batches.iter().map(|batch| batch.num_rows()).sum())
}

fn failed_iteration(error: impl std::fmt::Display) -> QueryIteration {
    QueryIteration {
        row_count: 0,
        elapsed: 0.0,
        error: Some(error.to_string()),
    }
}

async fn register_tables(ctx: &SessionContext, dataset_path: &Path) -> Result<()> {
    let mut table_paths = fs::read_dir(dataset_path)
        .with_context(|| format!("read dataset directory {}", dataset_path.display()))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    table_paths.sort();

    if table_paths.is_empty() {
        bail!(
            "dataset directory {} has no table directories",
            dataset_path.display()
        );
    }

    for path in table_paths {
        let table_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("invalid table directory {}", path.display()))?;
        let table_path = collection_path(&path);
        ctx.register_parquet(table_name, &table_path, ParquetReadOptions::default())
            .await
            .with_context(|| format!("register Parquet table {table_name} from {table_path}"))?;
    }
    Ok(())
}

async fn apply_query_settings(ctx: &SessionContext, query_sql: &str) -> Result<()> {
    for statement in query_sql.lines().filter_map(|line| {
        let directive = line.trim().strip_prefix("--")?.trim_start();
        let setting = directive.strip_prefix("set ")?;
        let setting = setting.trim().trim_end_matches(';').trim();
        (!setting.is_empty()).then(|| format!("SET {setting}"))
    }) {
        ctx.sql(&statement).await?;
    }
    Ok(())
}

fn selected_queries(query_dir: &Path, requested: &[String]) -> Result<Vec<String>> {
    if !requested.is_empty() {
        for query in requested {
            let path = query_dir.join(format!("{query}.sql"));
            if !path.is_file() {
                bail!("query file does not exist: {}", path.display());
            }
        }
        return Ok(requested.to_vec());
    }

    let mut queries = fs::read_dir(query_dir)?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|extension| extension.to_str()) == Some("sql"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .collect::<Vec<_>>();
    queries.sort_by_key(|query| query_sort_key(query));
    Ok(queries)
}

fn query_sort_key(query: &str) -> (u32, String) {
    let number = query
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(u32::MAX);
    (number, query.to_string())
}

fn statements(sql: &str) -> impl Iterator<Item = &str> {
    sql.split(';').map(str::trim).filter(|sql| !sql.is_empty())
}

fn is_setup_statement(sql: &str) -> bool {
    let sql = sql.trim_start().to_ascii_lowercase();
    sql.starts_with("create") || sql.starts_with("drop")
}

fn collection_path(path: &Path) -> String {
    format!("{}/", path.display())
}

fn validate_dir(path: &Path, kind: &str) -> Result<()> {
    if !path.is_dir() {
        bail!("{kind} directory does not exist: {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{collection_path, is_setup_statement, query_sort_key, statements};
    use std::path::Path;

    #[test]
    fn query_sorting_is_numeric() {
        let mut queries = vec!["q10", "q2", "q1"];
        queries.sort_by_key(|query| query_sort_key(query));
        assert_eq!(queries, ["q1", "q2", "q10"]);
    }

    #[test]
    fn collection_paths_end_in_a_slash() {
        assert_eq!(
            collection_path(Path::new("/tmp/lineitem")),
            "/tmp/lineitem/"
        );
    }

    #[test]
    fn statements_ignore_empty_trailing_segments() {
        assert_eq!(statements("select 1; ;").collect::<Vec<_>>(), ["select 1"]);
        assert!(is_setup_statement(" CREATE VIEW v AS SELECT 1"));
        assert!(!is_setup_statement("SELECT * FROM v"));
    }
}
