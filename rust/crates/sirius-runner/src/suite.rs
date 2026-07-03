use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use include_dir::{Dir, include_dir};
use serde::{Deserialize, Serialize};

static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/suites");

/// A benchmark suite definition (`suite.toml`): the queries to run, the
/// logical dataset they need, and how to run them. Datasets are specs, not
/// paths — the runner resolves them under the data root and generates them
/// when missing, so suites stay machine-portable.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteManifest {
    pub suite: SuiteMeta,
    pub dataset: DatasetSpec,
    #[serde(default)]
    pub engine: EngineSpec,
    pub run: RunSpec,
    #[serde(default)]
    pub validation: ValidationSpec,
    pub queries: Vec<QuerySpec>,
}

impl SuiteManifest {
    pub fn parse(manifest: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(manifest)?)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuiteMeta {
    pub name: String,
    pub description: Option<String>,
}

/// Logical dataset spec; keys into the data root by every data-affecting
/// property: `<data-root>/<benchmark>/sf<N>/<format>[-<compression>][-<encoding>]/`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetSpec {
    pub benchmark: String,
    pub scale_factor: f64,
    pub format: DataFormat,
    pub compression: Option<String>,
    pub encoding: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineSpec {
    /// Sirius engine config (YAML), relative to the suite directory. Falls
    /// back to the engine's own SIRIUS_CONFIG_FILE resolution.
    pub config: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub iterations: u32,
    pub mode: RunMode,
    #[serde(default)]
    pub engine: Engine,
    pub timeout_s: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationSpec {
    /// Expected-results directory relative to the suite directory, produced
    /// by `sirius-runner validate generate`.
    pub expected: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuerySpec {
    pub name: String,
    /// Query SQL file relative to the suite directory.
    pub sql: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, clap::ValueEnum)]
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

/// Suite source: the set embedded at compile time, or a directory override
/// (--suites / SIRIUS_RUNNER_SUITES). Layout either way:
/// `<root>/<name>/suite.toml` plus the files it references.
pub enum Suites {
    Embedded,
    Dir(PathBuf),
}

impl Suites {
    pub fn resolve(dir: Option<&Path>) -> Self {
        match dir {
            Some(dir) => Self::Dir(dir.to_path_buf()),
            None => Self::Embedded,
        }
    }

    pub fn names(&self) -> anyhow::Result<Vec<String>> {
        let mut names = match self {
            Self::Embedded => EMBEDDED
                .dirs()
                .filter(|dir| dir.contains(dir.path().join("suite.toml")))
                .map(|dir| dir.path().display().to_string())
                .collect::<Vec<_>>(),
            Self::Dir(root) => fs::read_dir(root)
                .with_context(|| format!("reading suites directory {}", root.display()))?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("suite.toml").is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect(),
        };
        names.sort();
        Ok(names)
    }

    pub fn load(&self, name: &str) -> anyhow::Result<SuiteManifest> {
        let manifest = SuiteManifest::parse(&self.read(name, Path::new("suite.toml"))?)
            .with_context(|| format!("parsing manifest of suite `{name}`"))?;
        if manifest.suite.name != name {
            bail!(
                "suite `{name}` declares mismatching name `{}`",
                manifest.suite.name
            );
        }
        Ok(manifest)
    }

    /// Read a file belonging to suite `name`, addressed relative to its
    /// directory.
    pub fn read(&self, name: &str, file: &Path) -> anyhow::Result<String> {
        let relative = Path::new(name).join(file);
        match self {
            Self::Embedded => Ok(EMBEDDED
                .get_file(&relative)
                .with_context(|| format!("no embedded suite file {}", relative.display()))?
                .contents_utf8()
                .with_context(|| format!("{} is not UTF-8", relative.display()))?
                .to_string()),
            Self::Dir(root) => {
                let path = root.join(&relative);
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tpch_suite() {
        let suites = Suites::Embedded;
        assert_eq!(suites.names().unwrap(), ["tpch"]);

        let tpch = suites.load("tpch").unwrap();
        assert_eq!(tpch.queries.len(), 22);

        // Every referenced query file must be embedded and non-empty.
        for query in &tpch.queries {
            let sql = suites.read(&tpch.suite.name, &query.sql).unwrap();
            assert!(!sql.trim().is_empty(), "{} is empty", query.name);
        }
    }
}
