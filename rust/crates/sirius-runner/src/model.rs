use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::assets::{CompareStrategy, DataFormat, Engine};

pub const PLAN_SCHEMA_VERSION: u32 = 2;
pub const DATASET_FORMAT_VERSION: u32 = 1;
pub const RUN_RESULT_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlan {
    pub schema_version: u32,
    pub runner_version: String,
    pub runner_sha256: String,
    pub origin: ExecutionOrigin,
    pub repository: RepositoryIdentity,
    pub benchmark: BenchmarkPlan,
    pub repo_root: PathBuf,
    pub data_root: PathBuf,
    pub output_dir: PathBuf,
    pub dataset: DatasetPlan,
    pub build: BuildPlan,
    pub queries: Vec<ResolvedQuery>,
    pub engines: Vec<Engine>,
    pub execution: ExecutionPlan,
    pub config: Option<PathBuf>,
    pub pin: PinPolicy,
    pub sources: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExecutionOrigin {
    Local,
    Ssh {
        target: String,
        run_id: String,
        remote_repo: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkPlan {
    pub name: String,
    pub description: Option<String>,
    pub suite: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetSpec {
    pub schema_version: u32,
    pub kind: String,
    pub generator: String,
    pub generator_version: u32,
    pub scale_factor: f64,
    pub format: DataFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetPlan {
    pub spec: DatasetSpec,
    pub recipe: Option<DatasetRecipe>,
    pub id: String,
    pub entry_dir: PathBuf,
    pub data_path: PathBuf,
    pub cache: CacheState,
    pub estimated_bytes: u64,
    pub managed: bool,
    pub verify_content: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetRecipe {
    pub schema_version: u32,
    pub wrapper_sha256: String,
    pub tpchgen_revision: String,
    pub jobs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheState {
    Hit,
    Miss,
    Invalid,
    External,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPlan {
    pub preset: Option<String>,
    pub build_dir: PathBuf,
    pub duckdb_binary: PathBuf,
    pub extension: PathBuf,
    pub action: BuildAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildAction {
    IncrementalBuild,
    UseExisting,
    NotRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedQuery {
    pub name: String,
    pub sql: String,
    pub sql_sha256: String,
    pub validation: QueryValidationPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryValidationPlan {
    pub compare: CompareStrategy,
    pub float_tolerance: Option<f64>,
    pub ordered_results: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionPlan {
    pub warmups: u32,
    pub iterations: u32,
    pub timeout_s: u64,
    pub validate: bool,
    pub cache_state: String,
    pub timing_boundary: String,
    pub trial_order: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum PinPolicy {
    #[default]
    None,
    Gpu,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentity {
    pub duckdb_binary_sha256: String,
    pub extension_sha256: String,
    pub config_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceIdentity {
    pub duckdb_version: String,
    pub duckdb_threads: u32,
    pub preserve_insertion_order: bool,
    pub module_path: PathBuf,
    pub module_sha256: String,
    pub python_version: String,
    pub python_executable: PathBuf,
    pub python_executable_sha256: String,
    pub worker_sha256: String,
}
