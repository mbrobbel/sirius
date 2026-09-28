use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    str::FromStr,
};

pub use crate::name::Name;

#[derive(Clone, Debug)]
pub enum Selection {
    All,
    One(Name),
}
impl FromStr for Selection {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if s == "all" {
            Ok(Self::All)
        } else {
            Ok(Self::One(s.parse()?))
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum All {
    #[default]
    All,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Choices {
    All(All),
    Names(Vec<Name>),
}
impl Default for Choices {
    fn default() -> Self {
        Self::All(All::All)
    }
}
impl FromStr for Choices {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if s == "all" {
            Ok(Self::default())
        } else {
            Ok(Self::Names(
                s.split(',').map(str::parse).collect::<Result<_>>()?,
            ))
        }
    }
}
impl Choices {
    fn resolve<T>(&self, registry: &BTreeMap<Name, T>, kind: &str) -> Result<Vec<Name>> {
        let names = match self {
            Self::All(_) => registry.keys().cloned().collect(),
            Self::Names(names) => names.clone(),
        };
        references(&names, registry, kind, true)?;
        Ok(names)
    }
}

#[derive(Clone, Debug)]
pub struct AxisOverride {
    pub name: Name,
    pub choices: Choices,
}
impl FromStr for AxisOverride {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        let (name, choices) = s
            .split_once('=')
            .context("axis must be NAME=CHOICE[,CHOICE] or NAME=all")?;
        Ok(Self {
            name: name.parse()?,
            choices: choices.parse()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PathBinding {
    pub name: Name,
    pub path: PathBuf,
}
impl FromStr for PathBinding {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let (name, path) = value.split_once('=').context("binding must be NAME=PATH")?;
        ensure!(!path.is_empty(), "binding path must not be empty");
        Ok(Self {
            name: name.parse()?,
            path: path.into(),
        })
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checkpoint {
    #[default]
    Automatic,
    Explicit,
}


#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub gpus: NonZeroUsize,
    pub config: PathBuf,
    #[serde(default)]
    pub environment: Environment,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<String, String>",
    into = "BTreeMap<String, String>"
)]
pub struct Environment(BTreeMap<String, String>);

impl TryFrom<BTreeMap<String, String>> for Environment {
    type Error = anyhow::Error;
    fn try_from(values: BTreeMap<String, String>) -> Result<Self> {
        for (name, value) in &values {
            ensure!(
                name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "invalid environment variable {name:?}"
            );
            ensure!(
                !matches!(name.as_str(), "SIRIUS_CONFIG_FILE" | "SIRIUS_DISABLE"),
                "environment variable {name} is owned by the runner"
            );
            ensure!(
                !value.contains('\0'),
                "environment variable {name} contains NUL"
            );
        }
        Ok(Self(values))
    }
}

impl From<Environment> for BTreeMap<String, String> {
    fn from(value: Environment) -> Self {
        value.0
    }
}

impl Environment {
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Table,
    View,
}
impl Relation {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Table => "TABLE",
            Self::View => "VIEW",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub relation: Relation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Run {
    Correctness(CorrectnessRun),
    Sweep(Matrix),
}

impl Run {
    fn selection(&self) -> (&Choices, &[Name]) {
        match self {
            Self::Correctness(run) => (&run.suites, &run.exclude_suites),
            Self::Sweep(run) => (&run.suites, &run.exclude_suites),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectnessRun {
    pub suites: Choices,
    #[serde(default)]
    pub exclude_suites: Vec<Name>,
    pub profile: Name,
    #[serde(default)]
    pub storage: Choices,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub reference_settings: Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub suites: Choices,
    #[serde(default)]
    pub exclude_suites: Vec<Name>,
    pub axes: BTreeMap<Name, Choices>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Boolean(bool),
    Integer(i64),
    Float(f64),
    String(String),
}
impl SettingValue {
    fn sql(&self) -> String {
        match self {
            Self::Boolean(v) => v.to_string(),
            Self::Integer(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::String(v) => crate::worker::quote(v),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(
    try_from = "BTreeMap<Name, SettingValue>",
    into = "BTreeMap<Name, SettingValue>"
)]
pub struct Settings(BTreeMap<Name, SettingValue>);
impl TryFrom<BTreeMap<Name, SettingValue>> for Settings {
    type Error = anyhow::Error;
    fn try_from(values: BTreeMap<Name, SettingValue>) -> Result<Self> {
        for (key, value) in &values {
            ensure!(
                key.as_ref() == key.as_ref().to_ascii_lowercase(),
                "setting names must be lowercase: {key}"
            );
            ensure!(
                !matches!(key.as_ref(), "gpu_execution" | "enable_duckdb_fallback"),
                "{key} is controlled by the runner"
            );
            ensure!(
                !matches!(value, SettingValue::Float(v) if !v.is_finite()),
                "setting {key} must be finite"
            );
        }
        Ok(Self(values))
    }
}
impl From<Settings> for BTreeMap<Name, SettingValue> {
    fn from(value: Settings) -> Self {
        value.0
    }
}
impl Settings {
    fn merge(&mut self, other: &Self) -> Result<()> {
        for (key, value) in &other.0 {
            if let Some(existing) = self.0.get(key) {
                ensure!(existing == value, "conflicting values for setting {key}");
            }
            self.0.insert(key.clone(), value.clone());
        }
        Ok(())
    }
    pub fn sql(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|(name, value)| format!("SET \"{name}\" = {};", value.sql()))
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisChoice {
    pub profile: Option<Name>,
    pub storage: Option<Name>,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub reference_settings: Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Fixture {
    Sql(SqlFixture),
    Files(FileFixture),
}

impl Fixture {
    pub fn extensions(&self) -> &[Name] {
        match self {
            Self::Sql(fixture) => &fixture.extensions,
            Self::Files(fixture) => &fixture.extensions,
        }
    }

    pub fn sources(&self) -> Vec<&Name> {
        match self {
            Self::Sql(fixture) => fixture.sources.iter().collect(),
            Self::Files(fixture) => fixture
                .files
                .values()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        }
    }

    pub fn outputs(&self) -> Vec<PathBuf> {
        match self {
            Self::Sql(fixture) => fixture
                .tables
                .iter()
                .map(|table| PathBuf::from(format!("{table}.parquet")))
                .collect(),
            Self::Files(fixture) => fixture.files.keys().map(|path| path.0.clone()).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParquetCompression {
    #[default]
    Zstd,
    Snappy,
}

impl ParquetCompression {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn sql(self) -> &'static str {
        match self {
            Self::Zstd => "ZSTD",
            Self::Snappy => "SNAPPY",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqlFixture {
    #[serde(default, skip_serializing_if = "ParquetCompression::is_default")]
    pub compression: ParquetCompression,
    #[serde(default)]
    pub extensions: Vec<Name>,
    pub sql: Vec<PathBuf>,
    pub tables: Vec<Name>,
    #[serde(default)]
    pub sources: Vec<Name>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileFixture {
    pub files: BTreeMap<FixturePath, Name>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<Name>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct FixturePath(PathBuf);

impl TryFrom<PathBuf> for FixturePath {
    type Error = anyhow::Error;

    fn try_from(path: PathBuf) -> Result<Self> {
        ensure!(
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_))),
            "fixture output must be a relative path without '.' or '..': {}",
            path.display()
        );
        Ok(Self(path.components().collect()))
    }
}

impl From<FixturePath> for PathBuf {
    fn from(path: FixturePath) -> Self {
        path.0
    }
}

impl AsRef<Path> for FixturePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    Pinned(PinnedSource),
    Provided(ProvidedSource),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PinnedSource {
    pub sha256: Digest,
    #[serde(flatten)]
    pub location: SourceLocation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProvidedSource {
    Provided,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceLocation {
    File {
        path: PathBuf,
    },
    Download {
        url: String,
        compression: Compression,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Compression {
    None,
    Gzip,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Digest(String);
impl TryFrom<String> for Digest {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit()),
            "expected a SHA-256 digest"
        );
        Ok(Self(value.to_ascii_lowercase()))
    }
}
impl From<Digest> for String {
    fn from(value: Digest) -> Self {
        value.0
    }
}
impl AsRef<str> for Digest {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Import {
    #[serde(default)]
    pub extensions: Vec<Name>,
    pub schema: Vec<PathBuf>,
    pub source: QuerySource,
    pub expected_queries: NonZeroUsize,
    pub provenance: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuerySource {
    Database { query: String },
    SqlLines { path: PathBuf },
}

fn references<T>(
    names: &[Name],
    registry: &BTreeMap<Name, T>,
    kind: &str,
    required: bool,
) -> Result<()> {
    ensure!(
        !required || !names.is_empty(),
        "{kind} list must not be empty"
    );
    let mut seen = BTreeSet::new();
    for name in names {
        ensure!(registry.contains_key(name), "unknown {kind} {name}");
        ensure!(seen.insert(name), "duplicate {kind} {name}");
    }
    Ok(())
}
