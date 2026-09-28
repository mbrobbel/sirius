use anyhow::{Context, Result, ensure};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub default_run: Name,
    pub suite_roots: Vec<PathBuf>,
    pub profiles: BTreeMap<Name, Profile>,
    pub storage: BTreeMap<Name, Storage>,
    #[serde(default)]
    pub axes: BTreeMap<Name, BTreeMap<Name, AxisChoice>>,
    pub runs: BTreeMap<Name, Run>,
    #[serde(default)]
    pub fixtures: BTreeMap<Name, Fixture>,
    #[serde(default)]
    pub sources: BTreeMap<Name, Source>,
    #[serde(default)]
    pub services: BTreeMap<Name, crate::services::Service>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Checkpoint {
    #[default]
    Automatic,
    Explicit,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteSpec {
    #[serde(default)]
    substitutions: crate::substitutions::Substitutions,
    object_store: Option<Name>,
    profile: Option<Name>,
    minimum_gpus: Option<NonZeroUsize>,
    #[serde(default)]
    scratch_directories: Vec<FixturePath>,
    #[serde(default)]
    fixture_files: BTreeMap<FixturePath, Name>,
    #[serde(default)]
    checkpoint: Checkpoint,
    #[serde(default)]
    storage: Choices,
    #[serde(default)]
    fixtures: Vec<Name>,
    import: Option<Import>,
}

impl SuiteSpec {
    fn load(directory: &Path) -> Result<Self> {
        let path = directory.join("suite.toml");
        if path.exists() {
            toml::from_str(&fs::read_to_string(&path)?)
                .with_context(|| format!("parse {}", path.display()))
        } else {
            Ok(Self::default())
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Suite {
    pub substitutions: crate::substitutions::Substitutions,
    pub object_store: Option<Name>,
    pub profile: Option<Name>,
    pub minimum_gpus: Option<NonZeroUsize>,
    pub scratch_directories: Vec<FixturePath>,
    pub fixture_files: BTreeMap<FixturePath, Name>,
    pub checkpoint: Checkpoint,
    pub directory: PathBuf,
    pub storage: Vec<Name>,
    pub fixtures: Vec<Name>,
    pub import: Option<Import>,
}

impl Suite {
    pub fn write_reproduction(&self, path: &Path) -> Result<()> {
        let spec = SuiteSpec {
            substitutions: self.substitutions.clone(),
            object_store: self.object_store.clone(),
            profile: self.profile.clone(),
            minimum_gpus: self.minimum_gpus,
            scratch_directories: self.scratch_directories.clone(),
            fixture_files: self.fixture_files.clone(),
            checkpoint: self.checkpoint,
            storage: Choices::Names(self.storage.clone()),
            fixtures: self.fixtures.clone(),
            import: None,
        };
        fs::write(path, toml::to_string(&spec)?)?;
        Ok(())
    }

    pub fn script_setup(&self) -> ScriptSetup {
        ScriptSetup {
            substitutions: self.substitutions.clone(),
            object_store: self.object_store.clone(),
            checkpoint: self.checkpoint,
            scratch_directories: self.scratch_directories.clone(),
            fixture_files: self.fixture_files.clone(),
            fixtures: self.fixtures.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScriptSetup {
    pub substitutions: crate::substitutions::Substitutions,
    pub object_store: Option<Name>,
    pub checkpoint: Checkpoint,
    pub scratch_directories: Vec<FixturePath>,
    pub fixture_files: BTreeMap<FixturePath, Name>,
    pub fixtures: Vec<Name>,
}

impl ScriptSetup {
    fn from_spec(spec: &SuiteSpec, fixtures: &BTreeMap<Name, Fixture>) -> Result<Self> {
        crate::substitutions::validate(&spec.substitutions, spec.object_store.is_some())?;
        references(&spec.fixtures, fixtures, "fixture", false)?;
        let mut required = spec.fixtures.clone();
        for (path, name) in &spec.fixture_files {
            ensure!(fixtures.contains_key(name), "unknown fixture {name}");
            if !required.contains(name) {
                required.push(name.clone());
            }
            for other in spec.fixture_files.keys() {
                ensure!(
                    path == other || !path.as_ref().starts_with(other),
                    "fixture copy destinations must not overlap: {} and {}",
                    path.as_ref().display(),
                    other.as_ref().display()
                );
            }
        }
        Ok(Self {
            substitutions: spec.substitutions.clone(),
            object_store: spec.object_store.clone(),
            checkpoint: spec.checkpoint,
            scratch_directories: spec.scratch_directories.clone(),
            fixture_files: spec.fixture_files.clone(),
            fixtures: required,
        })
    }
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

pub struct Config {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub suites: BTreeMap<Name, Suite>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Target {
    pub checkpoint: Checkpoint,
    pub suite: Name,
    pub profile: Name,
    pub storage: Name,
    pub axes: BTreeMap<Name, Name>,
    pub settings: Settings,
    pub reference_settings: Settings,
}
impl Target {
    pub fn label(&self) -> String {
        if self.axes.is_empty() {
            format!("{} / {}", self.profile, self.storage)
        } else {
            axis_label(&self.axes)
        }
    }
}
pub fn axis_label(axes: &BTreeMap<Name, Name>) -> String {
    axes.iter()
        .map(|(axis, choice)| format!("{axis}={choice}"))
        .join(", ")
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub run: Name,
    pub targets: Vec<Target>,
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

fn merge_dimension(target: &mut Option<Name>, value: &Option<Name>, dimension: &str) -> Result<()> {
    if let Some(value) = value {
        ensure!(
            target.as_ref().is_none_or(|old| old == value),
            "conflicting {dimension} choices in sweep"
        );
        *target = Some(value.clone());
    }
    Ok(())
}

impl Config {
    pub fn load(root: &Path) -> Result<Self> {
        Self::load_with_suites(root, &[])
    }

    pub fn load_with_suites(root: &Path, extra: &[PathBinding]) -> Result<Self> {
        let root = root.canonicalize()?;
        let path = root.join("sqltest.toml");
        let manifest: Manifest = toml::from_str(&fs::read_to_string(&path)?)
            .with_context(|| format!("parse {}", path.display()))?;
        ensure!(
            manifest.version == 1,
            "unsupported sqltest.toml version {}",
            manifest.version
        );
        ensure!(
            manifest.runs.contains_key(&manifest.default_run),
            "unknown default run {}",
            manifest.default_run
        );
        let mut config = Self {
            root,
            manifest,
            suites: BTreeMap::new(),
        };
        for path in config.manifest.suite_roots.clone() {
            let directory = config.path(&path)?;
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let path = entry.path();
                let mut has_tests = path.join("suite.toml").is_file();
                for entry in walkdir::WalkDir::new(&path) {
                    let entry = entry?;
                    if entry.file_type().is_file()
                        && entry.path().extension().is_some_and(|s| s == "slt")
                    {
                        has_tests = true;
                        break;
                    }
                }
                if has_tests {
                    config.add_suite(
                        entry
                            .file_name()
                            .to_str()
                            .context("suite name is not UTF-8")?
                            .parse()?,
                        &path,
                    )?;
                }
            }
        }
        for binding in extra {
            config.add_suite(binding.name.clone(), &binding.path)?;
        }
        for (name, run) in &config.manifest.runs {
            let (suites, excluded) = run.selection();
            suites
                .resolve(&config.suites, "suite")
                .with_context(|| format!("run {name}"))?;
            references(excluded, &config.suites, "excluded suite", false)?;
            let matrix = match run {
                Run::Correctness(run) => {
                    references(
                        std::slice::from_ref(&run.profile),
                        &config.manifest.profiles,
                        "profile",
                        true,
                    )?;
                    run.storage.resolve(&config.manifest.storage, "storage")?;
                    continue;
                }
                Run::Sweep(matrix) => matrix,
            };
            ensure!(!matrix.axes.is_empty(), "run {name} has no axes");
            for (axis, choices) in &matrix.axes {
                let values = config
                    .manifest
                    .axes
                    .get(axis)
                    .with_context(|| format!("unknown axis {axis} in run {name}"))?;
                choices.resolve(values, &format!("choice for {axis}"))?;
            }
        }
        for (name, fixture) in &config.manifest.fixtures {
            let sources: Vec<_> = fixture.sources().into_iter().cloned().collect();
            references(&sources, &config.manifest.sources, "source", false)?;
            match fixture {
                Fixture::Sql(fixture) => {
                    ensure!(
                        !fixture.sql.is_empty() && !fixture.tables.is_empty(),
                        "fixture {name} needs SQL and exported tables"
                    );
                    ensure!(
                        fixture.tables.iter().collect::<BTreeSet<_>>().len()
                            == fixture.tables.len(),
                        "duplicate table in fixture {name}"
                    );
                    for sql in &fixture.sql {
                        ensure!(config.path(sql)?.is_file(), "fixture SQL must be a file");
                    }
                }
                Fixture::Files(fixture) => {
                    ensure!(!fixture.files.is_empty(), "fixture {name} needs files")
                }
            }
        }
        for service in config.manifest.services.values() {
            service.validate(&config.manifest.fixtures)?;
        }
        for profile in config.manifest.profiles.values() {
            ensure!(
                config.path(&profile.config)?.is_file(),
                "profile config must be a file"
            );
        }
        for (axis, choices) in &config.manifest.axes {
            ensure!(!choices.is_empty(), "axis {axis} has no choices");
            for choice in choices.values() {
                if let Some(profile) = &choice.profile {
                    references(
                        std::slice::from_ref(profile),
                        &config.manifest.profiles,
                        "profile",
                        true,
                    )?;
                }
                if let Some(storage) = &choice.storage {
                    references(
                        std::slice::from_ref(storage),
                        &config.manifest.storage,
                        "storage",
                        true,
                    )?;
                }
            }
        }
        Ok(config)
    }

    fn add_suite(&mut self, name: Name, directory: &Path) -> Result<()> {
        let directory = directory.canonicalize()?;
        ensure!(
            directory.is_dir(),
            "suite must be a directory: {}",
            directory.display()
        );
        let spec = SuiteSpec::load(&directory)?;
        let mut setup = ScriptSetup::from_spec(&spec, &self.manifest.fixtures)?;
        if let Some(name) = &spec.object_store {
            let service = self
                .manifest
                .services
                .get(name)
                .with_context(|| format!("unknown object store service {name}"))?;
            service.validate(&self.manifest.fixtures)?;
            setup.fixtures.extend(service.fixtures().cloned());
            setup.fixtures.sort();
            setup.fixtures.dedup();
        }
        if let Some(profile) = &spec.profile {
            references(
                std::slice::from_ref(profile),
                &self.manifest.profiles,
                "profile",
                true,
            )?;
        }
        let storage = spec.storage.resolve(&self.manifest.storage, "storage")?;
        if let Some(import) = &spec.import {
            for path in &import.schema {
                ensure!(self.path(path)?.is_file(), "import schema must be a file");
            }
            if let QuerySource::SqlLines { path } = &import.source {
                ensure!(self.path(path)?.is_file(), "query source must be a file");
            }
        }
        ensure!(
            !self.suites.contains_key(&name),
            "duplicate discovered suite {name}"
        );
        self.suites.insert(
            name,
            Suite {
                substitutions: spec.substitutions,
                object_store: spec.object_store,
                profile: spec.profile,
                minimum_gpus: spec.minimum_gpus,
                scratch_directories: spec.scratch_directories,
                fixture_files: spec.fixture_files,
                checkpoint: spec.checkpoint,
                directory,
                storage,
                fixtures: setup.fixtures,
                import: spec.import,
            },
        );
        Ok(())
    }

    pub fn script_setup_for(&self, file: &Path) -> Result<ScriptSetup> {
        if let Some(suite) = self
            .suites
            .values()
            .filter(|suite| file.starts_with(&suite.directory))
            .max_by_key(|suite| suite.directory.components().count())
        {
            return Ok(suite.script_setup());
        }
        // External suites and saved reproductions need no central registration.
        file.parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .find(|directory| directory.join("suite.toml").is_file())
            .map(|directory| {
                ScriptSetup::from_spec(&SuiteSpec::load(directory)?, &self.manifest.fixtures)
            })
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn path(&self, relative: &Path) -> Result<PathBuf> {
        ensure!(
            relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "manifest paths must be relative without '.' or '..': {}",
            relative.display()
        );
        let path = self.root.join(relative).canonicalize()?;
        ensure!(
            path.starts_with(&self.root),
            "manifest path escapes corpus root: {}",
            relative.display()
        );
        Ok(path)
    }

    pub fn suites(&self, run: Option<&Name>, selection: Option<&Selection>) -> Result<Vec<Name>> {
        let name = run.unwrap_or(&self.manifest.default_run);
        let run = self
            .manifest
            .runs
            .get(name)
            .with_context(|| format!("unknown run {name}"))?;
        let (configured, excluded) = run.selection();
        let choices = match selection {
            None => configured.clone(),
            Some(Selection::All) => Choices::default(),
            Some(Selection::One(name)) => Choices::Names(vec![name.clone()]),
        };
        let mut suites = choices.resolve(&self.suites, "suite")?;
        if selection.is_none() {
            suites.retain(|suite| !excluded.contains(suite));
        }
        ensure!(!suites.is_empty(), "run {name} selects no suites");
        Ok(suites)
    }

    pub fn plan(
        &self,
        run: Option<&Name>,
        suites: Option<&Selection>,
        overrides: &[AxisOverride],
    ) -> Result<Plan> {
        let name = run.unwrap_or(&self.manifest.default_run);
        let spec = self
            .manifest
            .runs
            .get(name)
            .with_context(|| format!("unknown run {name}"))?;
        let suites = self.suites(run, suites)?;
        let matrix = match spec {
            Run::Correctness(fixed) => {
                ensure!(
                    overrides.is_empty(),
                    "correctness runs use fixed suite profiles; select a sweep run for --axis overrides"
                );
                let storage = fixed.storage.resolve(&self.manifest.storage, "storage")?;
                let mut targets = Vec::new();
                for name in suites {
                    let suite = &self.suites[&name];
                    let profile = suite.profile.as_ref().unwrap_or(&fixed.profile);
                    if suite
                        .minimum_gpus
                        .is_some_and(|minimum| self.manifest.profiles[profile].gpus < minimum)
                    {
                        continue;
                    }
                    for mode in &suite.storage {
                        if storage.contains(mode) {
                            targets.push(Target {
                                checkpoint: suite.checkpoint,
                                suite: name.clone(),
                                profile: profile.clone(),
                                storage: mode.clone(),
                                axes: BTreeMap::new(),
                                settings: fixed.settings.clone(),
                                reference_settings: fixed.reference_settings.clone(),
                            });
                        }
                    }
                }
                ensure!(
                    !targets.is_empty(),
                    "run has no compatible suite/storage/GPU combinations"
                );
                return Ok(Plan {
                    run: name.clone(),
                    targets,
                });
            }
            Run::Sweep(matrix) => matrix,
        };
        let mut axes = matrix.axes.clone();
        let mut seen = BTreeSet::new();
        for value in overrides {
            ensure!(
                seen.insert(&value.name),
                "duplicate axis override {}",
                value.name
            );
            axes.insert(value.name.clone(), value.choices.clone());
        }
        let dimensions: Vec<Vec<_>> = axes
            .iter()
            .map(|(axis, selected)| {
                let choices = self
                    .manifest
                    .axes
                    .get(axis)
                    .with_context(|| format!("unknown axis {axis}"))?;
                Ok(selected
                    .resolve(choices, &format!("choice for {axis}"))?
                    .into_iter()
                    .map(|name| (axis.clone(), name.clone(), choices[&name].clone()))
                    .collect())
            })
            .collect::<Result<_>>()?;
        let mut targets = Vec::new();
        for combination in dimensions.into_iter().multi_cartesian_product() {
            let (mut profile, mut storage) = (None, None);
            let (mut settings, mut reference_settings) = (Settings::default(), Settings::default());
            let mut axes = BTreeMap::new();
            for (axis, name, choice) in combination {
                merge_dimension(&mut profile, &choice.profile, "profile")?;
                merge_dimension(&mut storage, &choice.storage, "storage")?;
                settings.merge(&choice.settings)?;
                reference_settings.merge(&choice.reference_settings)?;
                axes.insert(axis, name);
            }
            let profile = profile.context("sweep combination does not select a GPU profile")?;
            let storage = storage.context("sweep combination does not select a storage mode")?;
            for suite in &suites {
                if self.suites[suite].storage.contains(&storage)
                    && self.suites[suite]
                        .minimum_gpus
                        .is_none_or(|minimum| self.manifest.profiles[&profile].gpus >= minimum)
                {
                    targets.push(Target {
                        checkpoint: self.suites[suite].checkpoint,
                        suite: suite.clone(),
                        profile: profile.clone(),
                        storage: storage.clone(),
                        axes: axes.clone(),
                        settings: settings.clone(),
                        reference_settings: reference_settings.clone(),
                    });
                }
            }
        }
        ensure!(
            !targets.is_empty(),
            "run has no compatible suite/storage/GPU combinations"
        );
        Ok(Plan {
            run: name.clone(),
            targets,
        })
    }
}
