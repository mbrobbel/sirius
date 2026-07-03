//! Local results store (design stage): row types mirroring schema.sql.
//! No storage backend yet — the target is a local DuckDB file via duckdb-rs.

#![allow(dead_code)]

/// The results store.
pub struct Store;

impl Store {
    pub const SCHEMA: &'static str = include_str!("../schema.sql");
}

/// One row per distinct system-spec snapshot (`environments`).
#[derive(Debug)]
pub struct Environment {
    pub id: i64,
    pub hostname: String,
    pub os: Option<String>,
    pub cpu_model: Option<String>,
    pub cpu_cores: Option<i64>,
    pub ram_bytes: Option<i64>,
    pub gpu_name: Option<String>,
    pub gpu_memory_bytes: Option<i64>,
    pub gpu_driver: Option<String>,
    pub cuda_version: Option<String>,
    /// JSON: [{mount, total_bytes, free_bytes}]
    pub disks_json: Option<String>,
    pub collected_at: String,
}

/// One row per suite or bench invocation (`runs`).
#[derive(Debug)]
pub struct Run {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub environment_id: Option<i64>,
    pub commit_sha: Option<String>,
    pub branch: Option<String>,
    pub build_preset: Option<String>,
    /// sirius | duckdb (baseline)
    pub engine: String,
    /// Resolved engine config snapshot.
    pub engine_config_json: Option<String>,
    /// None for ad-hoc bench runs.
    pub suite: Option<String>,
    pub dataset_benchmark: Option<String>,
    pub scale_factor: Option<f64>,
    pub data_format: Option<String>,
    pub data_compression: Option<String>,
    pub data_encoding: Option<String>,
    /// cold | warm
    pub mode: Option<String>,
    pub iterations: Option<i64>,
    pub notes: Option<String>,
}

/// One row per (query, iteration) (`results`).
#[derive(Debug)]
pub struct QueryResult {
    pub id: i64,
    pub run_id: i64,
    pub query: String,
    pub iteration: i64,
    pub runtime_ms: Option<f64>,
    /// ok | error | timeout
    pub status: String,
    pub error: Option<String>,
    pub telemetry_path: Option<String>,
}

/// Correctness of a run's results against expected results (`validations`).
#[derive(Debug)]
pub struct Validation {
    pub id: i64,
    pub run_id: i64,
    pub query: String,
    /// match | mismatch | error
    pub status: String,
    pub detail: Option<String>,
}
