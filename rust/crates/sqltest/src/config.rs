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

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);

impl TryFrom<String> for Name {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            !value.is_empty()
                && value != "all"
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
            "invalid identifier {value:?}; use letters, digits, '_' or '-' (all is reserved)"
        );
        Ok(Self(value))
    }
}
impl From<Name> for String {
    fn from(value: Name) -> Self {
        value.0
    }
}
impl FromStr for Name {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::try_from(s.to_owned())
    }
}
impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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
    pub axes: BTreeMap<Name, BTreeMap<Name, AxisChoice>>,
    pub runs: BTreeMap<Name, Matrix>,
    #[serde(default)]
    pub fixtures: BTreeMap<Name, Fixture>,
    #[serde(default)]
    pub sources: BTreeMap<Name, Source>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SuiteSpec {
    #[serde(default)]
    storage: Choices,
    #[serde(default)]
    fixtures: Vec<Name>,
    import: Option<Import>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Suite {
    pub directory: PathBuf,
    pub storage: Vec<Name>,
    pub fixtures: Vec<Name>,
    pub import: Option<Import>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub gpus: NonZeroUsize,
    pub config: PathBuf,
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
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub suites: Choices,
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
#[serde(deny_unknown_fields)]
pub struct Fixture {
    #[serde(default)]
    pub extensions: Vec<Name>,
    pub sql: Vec<PathBuf>,
    pub tables: Vec<Name>,
    #[serde(default)]
    pub sources: Vec<Name>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub url: String,
    pub sha256: Digest,
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
    pub suite: Name,
    pub profile: Name,
    pub storage: Name,
    pub axes: BTreeMap<Name, Name>,
    pub settings: Settings,
    pub reference_settings: Settings,
}
impl Target {
    pub fn label(&self) -> String {
        axis_label(&self.axes)
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
        for (name, matrix) in &config.manifest.runs {
            matrix
                .suites
                .resolve(&config.suites, "suite")
                .with_context(|| format!("run {name}"))?;
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
            ensure!(
                !fixture.sql.is_empty() && !fixture.tables.is_empty(),
                "fixture {name} needs SQL and exported tables"
            );
            ensure!(
                fixture.tables.iter().collect::<BTreeSet<_>>().len() == fixture.tables.len(),
                "duplicate table in fixture {name}"
            );
            references(&fixture.sources, &config.manifest.sources, "source", false)?;
            for sql in &fixture.sql {
                ensure!(config.path(sql)?.is_file(), "fixture SQL must be a file");
            }
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
        let metadata = directory.join("suite.toml");
        let spec: SuiteSpec = if metadata.exists() {
            toml::from_str(&fs::read_to_string(&metadata)?)
                .with_context(|| format!("parse {}", metadata.display()))?
        } else {
            SuiteSpec::default()
        };
        references(&spec.fixtures, &self.manifest.fixtures, "fixture", false)?;
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
                directory,
                storage,
                fixtures: spec.fixtures,
                import: spec.import,
            },
        );
        Ok(())
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
        let matrix = self
            .manifest
            .runs
            .get(name)
            .with_context(|| format!("unknown run {name}"))?;
        let choices = match selection {
            None => matrix.suites.clone(),
            Some(Selection::All) => Choices::default(),
            Some(Selection::One(name)) => Choices::Names(vec![name.clone()]),
        };
        choices.resolve(&self.suites, "suite")
    }

    pub fn plan(
        &self,
        run: Option<&Name>,
        suites: Option<&Selection>,
        overrides: &[AxisOverride],
    ) -> Result<Plan> {
        let name = run.unwrap_or(&self.manifest.default_run);
        let matrix = self
            .manifest
            .runs
            .get(name)
            .with_context(|| format!("unknown run {name}"))?;
        let suites = self.suites(run, suites)?;
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
                if self.suites[suite].storage.contains(&storage) {
                    targets.push(Target {
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
            "run has no compatible suite/storage combinations"
        );
        Ok(Plan {
            run: name.clone(),
            targets,
        })
    }
}
