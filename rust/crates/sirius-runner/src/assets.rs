//! Benchmark definitions, layered so each piece is reusable:
//!
//! - `datasets/<name>.toml` — a dataset *family* (generator, formats); an
//!   *instance* adds logical args (scale factor) plus storage args (format,
//!   compression, encoding) and keys into the data root.
//! - `suites/<name>/suite.toml` — a query suite: queries over a dataset
//!   family, plus how to validate them. No instance args, no run params.
//! - `benches/<name>.toml` — a run configuration: suite + dataset instance
//!   args + engine selection + execution params. The executable notion.
//! - `expected/<suite>/sf<N>/` — validation data, keyed by the *logical*
//!   instance (independent of format/compression/encoding). Repo files, not
//!   embedded; see expected/README.md.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};

static DATASETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/datasets");
static SUITES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/suites");
static BENCHES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/benches");

/// A dataset family definition (`datasets/<name>.toml`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetManifest {
    pub dataset: DatasetMeta,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetMeta {
    pub name: String,
    pub description: Option<String>,
    /// How instances are produced, e.g. tpchgen, dbgen.
    pub generator: String,
    pub formats: Vec<DataFormat>,
}

/// A query suite definition (`suites/<name>/suite.toml`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteManifest {
    pub suite: SuiteMeta,
    #[serde(default)]
    pub validation: SuiteValidation,
    pub queries: Vec<QuerySpec>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteMeta {
    pub name: String,
    pub description: Option<String>,
    /// Dataset family the queries run against.
    pub dataset: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteValidation {
    /// Reference engine that generates expected results.
    pub reference: String,
}

impl Default for SuiteValidation {
    fn default() -> Self {
        Self {
            reference: "duckdb".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuerySpec {
    pub name: String,
    /// Query SQL file relative to the suite directory.
    pub sql: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<QueryValidation>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryValidation {
    #[serde(default)]
    pub compare: CompareStrategy,
    /// Tolerance for float comparison (rows strategy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float_tolerance: Option<f64>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareStrategy {
    /// Tolerance-aware row comparison against full expected results.
    #[default]
    Rows,
    /// Exact digest comparison; constant-size expected data, for queries
    /// whose result size scales with the dataset.
    Digest,
}

/// A run configuration (`benches/<name>.toml`): what CI, nightly, and
/// developers reference by name.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchManifest {
    pub bench: BenchMeta,
    pub dataset: DatasetParams,
    #[serde(default)]
    pub engine: EngineSelection,
    pub execution: ExecutionSpec,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchMeta {
    pub name: String,
    pub description: Option<String>,
    /// Query suite to run.
    pub suite: String,
}

/// Dataset instance args; the family comes from the suite. Together they key
/// into the data root as `<data-root>/<family>/sf<N>/<format>[-<compression>][-<encoding>]/`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetParams {
    pub scale_factor: f64,
    pub format: DataFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSelection {
    #[serde(default)]
    pub engine: Engine,
    /// Sirius engine config (YAML). Falls back to the engine's own
    /// SIRIUS_CONFIG_FILE resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    pub iterations: u32,
    pub mode: RunMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Validate results against expected results.
    #[serde(default)]
    pub validate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DataFormat {
    Parquet,
    Duckdb,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Cold,
    Warm,
}

/// Which engine(s) to run: Sirius on GPU, plain DuckDB as the CPU baseline,
/// or both.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Sirius,
    Duckdb,
    Both,
}

/// Asset source: the set embedded at compile time, or a directory override
/// (--assets / SIRIUS_RUNNER_ASSETS) with the same layout (`datasets/`,
/// `suites/`, `benches/`, `expected/`).
pub enum Assets {
    Embedded,
    Dir(PathBuf),
}

impl Assets {
    pub fn resolve(dir: Option<&Path>) -> Self {
        match dir {
            Some(dir) => Self::Dir(dir.to_path_buf()),
            None => Self::Embedded,
        }
    }

    pub fn dataset_names(&self) -> anyhow::Result<Vec<String>> {
        self.toml_stems(&DATASETS, "datasets")
    }

    pub fn suite_names(&self) -> anyhow::Result<Vec<String>> {
        let mut names = match self {
            Self::Embedded => SUITES
                .dirs()
                .filter(|dir| dir.contains(dir.path().join("suite.toml")))
                .map(|dir| dir.path().display().to_string())
                .collect::<Vec<_>>(),
            Self::Dir(root) => {
                let suites = root.join("suites");
                fs::read_dir(&suites)
                    .with_context(|| format!("reading {}", suites.display()))?
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().join("suite.toml").is_file())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            }
        };
        names.sort();
        Ok(names)
    }

    pub fn bench_names(&self) -> anyhow::Result<Vec<String>> {
        self.toml_stems(&BENCHES, "benches")
    }

    pub fn load_dataset(&self, name: &str) -> anyhow::Result<DatasetManifest> {
        let manifest: DatasetManifest = self.parse(&DATASETS, "datasets", name)?;
        if manifest.dataset.name != name {
            bail!(
                "dataset `{name}` declares mismatching name `{}`",
                manifest.dataset.name
            );
        }
        Ok(manifest)
    }

    pub fn load_suite(&self, name: &str) -> anyhow::Result<SuiteManifest> {
        let path = Path::new(name).join("suite.toml");
        let manifest: SuiteManifest = toml::from_str(&self.read(&SUITES, "suites", &path)?)
            .with_context(|| format!("parsing manifest of suite `{name}`"))?;
        if manifest.suite.name != name {
            bail!(
                "suite `{name}` declares mismatching name `{}`",
                manifest.suite.name
            );
        }
        Ok(manifest)
    }

    pub fn load_bench(&self, name: &str) -> anyhow::Result<BenchManifest> {
        let manifest: BenchManifest = self.parse(&BENCHES, "benches", name)?;
        if manifest.bench.name != name {
            bail!(
                "bench `{name}` declares mismatching name `{}`",
                manifest.bench.name
            );
        }
        Ok(manifest)
    }

    /// Read a file belonging to suite `name`, addressed relative to its
    /// directory.
    #[cfg_attr(not(test), expect(dead_code, reason = "used once bench run lands"))]
    pub fn read_suite_file(&self, name: &str, file: &Path) -> anyhow::Result<String> {
        self.read(&SUITES, "suites", &Path::new(name).join(file))
    }

    fn toml_stems(&self, embedded: &Dir<'_>, kind: &str) -> anyhow::Result<Vec<String>> {
        let stem = |path: &Path| {
            (path.extension().is_some_and(|ext| ext == "toml"))
                .then(|| path.file_stem().unwrap_or_default().display().to_string())
        };
        let mut names = match self {
            Self::Embedded => embedded
                .files()
                .filter_map(|file| stem(file.path()))
                .collect::<Vec<_>>(),
            Self::Dir(root) => {
                let dir = root.join(kind);
                fs::read_dir(&dir)
                    .with_context(|| format!("reading {}", dir.display()))?
                    .filter_map(Result::ok)
                    .filter_map(|entry| stem(&entry.path()))
                    .collect()
            }
        };
        names.sort();
        Ok(names)
    }

    fn parse<T: serde::de::DeserializeOwned>(
        &self,
        embedded: &Dir<'_>,
        kind: &str,
        name: &str,
    ) -> anyhow::Result<T> {
        let path = PathBuf::from(format!("{name}.toml"));
        toml::from_str(&self.read(embedded, kind, &path)?)
            .with_context(|| format!("parsing {kind}/{name}.toml"))
    }

    fn read(&self, embedded: &Dir<'_>, kind: &str, relative: &Path) -> anyhow::Result<String> {
        match self {
            Self::Embedded => Ok(embedded
                .get_file(relative)
                .with_context(|| format!("no embedded {kind} file {}", relative.display()))?
                .contents_utf8()
                .with_context(|| format!("{kind}/{} is not UTF-8", relative.display()))?
                .to_string()),
            Self::Dir(root) => {
                let path = root.join(kind).join(relative);
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_are_consistent() {
        let assets = Assets::Embedded;
        assert_eq!(assets.dataset_names().unwrap(), ["tpch"]);
        assert_eq!(assets.suite_names().unwrap(), ["tpch"]);
        assert_eq!(assets.bench_names().unwrap(), ["tpch-sf1", "tpch-sf100"]);

        // Every bench references an existing suite; every suite references an
        // existing dataset family and only formats that family supports.
        for name in assets.bench_names().unwrap() {
            let bench = assets.load_bench(&name).unwrap();
            let suite = assets.load_suite(&bench.bench.suite).unwrap();
            let dataset = assets.load_dataset(&suite.suite.dataset).unwrap();
            assert!(
                dataset.dataset.formats.contains(&bench.dataset.format),
                "bench {name} requests a format its dataset family lacks"
            );
        }

        // Every referenced query file must be embedded and non-empty.
        let tpch = assets.load_suite("tpch").unwrap();
        assert_eq!(tpch.queries.len(), 22);
        for query in &tpch.queries {
            let sql = assets.read_suite_file("tpch", &query.sql).unwrap();
            assert!(!sql.trim().is_empty(), "{} is empty", query.name);
        }
    }
}
