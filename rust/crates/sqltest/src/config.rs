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
    pub runs: BTreeMap<Name, Run>,
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
    profile: Option<Name>,
    minimum_gpus: Option<NonZeroUsize>,
    #[serde(default)]
    scratch_directories: Vec<FixturePath>,
    #[serde(default)]
    checkpoint: Checkpoint,
    #[serde(default)]
    storage: Choices,
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
    pub profile: Option<Name>,
    pub minimum_gpus: Option<NonZeroUsize>,
    pub scratch_directories: Vec<FixturePath>,
    pub checkpoint: Checkpoint,
    pub directory: PathBuf,
    pub storage: Vec<Name>,
}

impl Suite {
    pub fn write_reproduction(&self, path: &Path) -> Result<()> {
        let spec = SuiteSpec {
            substitutions: self.substitutions.clone(),

            profile: self.profile.clone(),
            minimum_gpus: self.minimum_gpus,
            scratch_directories: self.scratch_directories.clone(),

            checkpoint: self.checkpoint,
            storage: Choices::Names(self.storage.clone()),
        };
        fs::write(path, toml::to_string(&spec)?)?;
        Ok(())
    }

    pub fn script_setup(&self) -> ScriptSetup {
        ScriptSetup {
            substitutions: self.substitutions.clone(),

            checkpoint: self.checkpoint,
            scratch_directories: self.scratch_directories.clone(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScriptSetup {
    pub substitutions: crate::substitutions::Substitutions,
    pub checkpoint: Checkpoint,
    pub scratch_directories: Vec<FixturePath>,
}

impl ScriptSetup {
    fn from_spec(spec: &SuiteSpec) -> Result<Self> {
        crate::substitutions::validate(&spec.substitutions, false)?;
        Ok(Self {
            substitutions: spec.substitutions.clone(),

            checkpoint: spec.checkpoint,
            scratch_directories: spec.scratch_directories.clone(),
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
}

impl Run {
    fn selection(&self) -> (&Choices, &[Name]) {
        match self {
            Self::Correctness(run) => (&run.suites, &run.exclude_suites),
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
    pub fn sql(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|(name, value)| format!("SET \"{name}\" = {};", value.sql()))
            .collect()
    }
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
            let Run::Correctness(run) = run;
            references(
                std::slice::from_ref(&run.profile),
                &config.manifest.profiles,
                "profile",
                true,
            )?;
            run.storage.resolve(&config.manifest.storage, "storage")?;
        }
        for profile in config.manifest.profiles.values() {
            ensure!(
                config.path(&profile.config)?.is_file(),
                "profile config must be a file"
            );
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
        ScriptSetup::from_spec(&spec)?;
        if let Some(profile) = &spec.profile {
            references(
                std::slice::from_ref(profile),
                &self.manifest.profiles,
                "profile",
                true,
            )?;
        }
        let storage = spec.storage.resolve(&self.manifest.storage, "storage")?;
        ensure!(
            !self.suites.contains_key(&name),
            "duplicate discovered suite {name}"
        );
        self.suites.insert(
            name,
            Suite {
                substitutions: spec.substitutions,

                profile: spec.profile,
                minimum_gpus: spec.minimum_gpus,
                scratch_directories: spec.scratch_directories,

                checkpoint: spec.checkpoint,
                directory,
                storage,
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
            .map(|directory| ScriptSetup::from_spec(&SuiteSpec::load(directory)?))
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

    pub fn plan(&self, run: Option<&Name>, suites: Option<&Selection>) -> Result<Plan> {
        let name = run.unwrap_or(&self.manifest.default_run);
        let spec = self
            .manifest
            .runs
            .get(name)
            .with_context(|| format!("unknown run {name}"))?;
        let suites = self.suites(run, suites)?;
        let Run::Correctness(fixed) = spec;
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
        Ok(Plan {
            run: name.clone(),
            targets,
        })
    }
}
