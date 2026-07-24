//! Benchmark definitions, layered so each piece is reusable:
//!
//! - `datasets/<name>.toml` — a dataset *family* (generator, formats); an
//!   *instance* adds a scale factor and storage format and keys into the data
//!   root.
//! - `suites/<name>/suite.toml` — a query suite: queries over a dataset
//!   family, plus how to validate them. No instance args, no run params.
//! - `benches/<name>.toml` — a run configuration: suite + dataset instance
//!   args + engine selection + execution params. The executable notion.
//! - Expected results are generated with DuckDB and cached by the complete
//!   dataset, query, worker, and reference-runtime identity.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, bail};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};

static DATASETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/datasets");
static SUITES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/suites");
static BENCHES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/benches");

/// A dataset family definition (`datasets/<name>.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetManifest {
    pub dataset: DatasetMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetMeta {
    pub name: String,
    pub description: Option<String>,
    /// How instances are produced, e.g. tpchgen, dbgen.
    pub generator: String,
    pub formats: Vec<DataFormat>,
}

/// A query suite definition (`suites/<name>/suite.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteManifest {
    pub suite: SuiteMeta,
    #[serde(default)]
    pub validation: SuiteValidation,
    pub queries: Vec<QuerySpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteMeta {
    pub name: String,
    pub description: Option<String>,
    /// Dataset family the queries run against.
    pub dataset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteValidation {
    /// Reference engine that generates expected results.
    pub reference: String,
    /// Whether query result order is significant unless a query overrides it.
    #[serde(default)]
    pub ordered_results: bool,
}

impl Default for SuiteValidation {
    fn default() -> Self {
        Self {
            reference: "duckdb".to_string(),
            ordered_results: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuerySpec {
    pub name: String,
    /// Query SQL file relative to the suite directory.
    pub sql: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<QueryValidation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryValidation {
    #[serde(default)]
    pub compare: CompareStrategy,
    /// Tolerance for float comparison (rows strategy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub float_tolerance: Option<f64>,
    /// Override the suite's result-order policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordered_results: Option<bool>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareStrategy {
    /// Tolerance-aware row comparison against full expected results.
    #[default]
    Rows,
    /// Exact digest comparison.
    Digest,
}

/// A run configuration (`benches/<name>.toml`): what CI, nightly, and
/// developers reference by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchManifest {
    pub bench: BenchMeta,
    pub dataset: DatasetParams,
    #[serde(default)]
    pub engine: EngineSelection,
    pub execution: ExecutionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BenchMeta {
    pub name: String,
    pub description: Option<String>,
    /// Query suite to run.
    pub suite: String,
}

/// Dataset instance args; the family comes from the suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetParams {
    pub scale_factor: f64,
    pub format: DataFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSelection {
    #[serde(default)]
    pub engine: Engine,
    /// Sirius engine config (YAML). Falls back to the engine's own
    /// SIRIUS_CONFIG_FILE resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    /// Untimed executions before measured iterations.
    #[serde(default = "default_warmups")]
    pub warmups: u32,
    pub iterations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Validate results against expected results.
    #[serde(default)]
    pub validate: bool,
}

const fn default_warmups() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DataFormat {
    Parquet,
    Duckdb,
}

/// Which engine(s) to run: Sirius on GPU, plain DuckDB as the CPU baseline,
/// or both.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    Sirius,
    Duckdb,
    Both,
}

/// Asset source: the set embedded at compile time, or a directory with the
/// same `datasets/`, `suites/`, and `benches/` layout (used by tests and
/// embedding applications).
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
        validate_dataset(&manifest)?;
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
        validate_suite(&manifest)?;
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
        validate_bench(&manifest)?;
        Ok(manifest)
    }

    /// Read a file belonging to suite `name`, addressed relative to its
    /// directory.
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
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("invalid {kind} asset path {}", relative.display());
        }
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

fn validate_dataset(manifest: &DatasetManifest) -> anyhow::Result<()> {
    if manifest.dataset.generator != "tpchgen" {
        bail!(
            "dataset `{}` uses unsupported generator `{}`; expected `tpchgen`",
            manifest.dataset.name,
            manifest.dataset.generator
        );
    }
    if manifest.dataset.formats.is_empty() {
        bail!("dataset `{}` has no formats", manifest.dataset.name);
    }
    Ok(())
}

fn validate_suite(manifest: &SuiteManifest) -> anyhow::Result<()> {
    if manifest.queries.is_empty() {
        bail!("suite `{}` has no queries", manifest.suite.name);
    }
    if manifest.validation.reference != "duckdb" {
        bail!(
            "suite `{}` uses unsupported reference engine `{}`; expected `duckdb`",
            manifest.suite.name,
            manifest.validation.reference
        );
    }
    let mut names = HashSet::new();
    for query in &manifest.queries {
        if query.name.trim().is_empty() {
            bail!(
                "suite `{}` contains an empty query name",
                manifest.suite.name
            );
        }
        if !names.insert(query.name.as_str()) {
            bail!(
                "suite `{}` contains duplicate query `{}`",
                manifest.suite.name,
                query.name
            );
        }
        if query.sql.is_absolute()
            || query
                .sql
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!(
                "query `{}` has unsafe SQL path {}",
                query.name,
                query.sql.display()
            );
        }
        if let Some(tolerance) = query
            .validation
            .as_ref()
            .and_then(|validation| validation.float_tolerance)
            && (!tolerance.is_finite() || tolerance < 0.0)
        {
            bail!("query `{}` has an invalid float tolerance", query.name);
        }
        if let Some(validation) = &query.validation
            && validation.compare == CompareStrategy::Digest
        {
            if validation.float_tolerance.is_some() {
                bail!(
                    "query `{}` cannot combine digest comparison with a float tolerance",
                    query.name
                );
            }
            let ordered = validation
                .ordered_results
                .unwrap_or(manifest.validation.ordered_results);
            if !ordered {
                bail!(
                    "query `{}` uses digest comparison and must declare ordered results",
                    query.name
                );
            }
        }
    }
    Ok(())
}

fn validate_bench(manifest: &BenchManifest) -> anyhow::Result<()> {
    let scale_factor = manifest.dataset.scale_factor;
    if !scale_factor.is_finite() || scale_factor <= 0.0 {
        bail!(
            "benchmark `{}` has an invalid scale factor",
            manifest.bench.name
        );
    }
    if manifest.execution.iterations == 0 {
        bail!(
            "benchmark `{}` must use at least one measured iteration",
            manifest.bench.name
        );
    }
    if manifest.execution.timeout_s == Some(0) {
        bail!(
            "benchmark `{}` must use a positive timeout",
            manifest.bench.name
        );
    }
    if manifest.dataset.compression.is_some() || manifest.dataset.encoding.is_some() {
        bail!(
            "benchmark `{}` requests compression or encoding overrides, which are not supported yet",
            manifest.bench.name
        );
    }
    Ok(())
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

    #[test]
    fn unsupported_manifest_capabilities_fail_instead_of_being_ignored() {
        let assets = Assets::Embedded;
        let mut dataset = assets.load_dataset("tpch").unwrap();
        dataset.dataset.generator = "other".to_string();
        assert!(validate_dataset(&dataset).is_err());

        let mut bench = assets.load_bench("tpch-sf1").unwrap();
        bench.dataset.compression = Some("zstd".to_string());
        assert!(validate_bench(&bench).is_err());
    }
}
