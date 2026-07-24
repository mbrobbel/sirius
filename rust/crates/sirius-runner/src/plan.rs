use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};

use crate::{
    assets::{Assets, BenchManifest, DataFormat, Engine},
    hashing,
    model::{
        BenchmarkPlan, BuildAction, BuildPlan, CacheState, DATASET_FORMAT_VERSION, DatasetPlan,
        DatasetSpec, ExecutionOrigin, ExecutionPlan, PLAN_SCHEMA_VERSION, PinPolicy,
        RepositoryIdentity, ResolvedQuery, RunPlan,
    },
};

const DEFAULT_TIMEOUT_S: u64 = 600;

#[derive(Debug, Clone)]
pub struct RunOverrides {
    pub name: String,
    pub repo_root: Option<PathBuf>,
    pub data_root: Option<PathBuf>,
    pub queries: Vec<String>,
    pub iterations: Option<u32>,
    pub engine: Option<Engine>,
    pub config: Option<PathBuf>,
    pub pin: PinPolicy,
    pub preset: Option<String>,
    pub build_dir: Option<PathBuf>,
    pub data: Option<PathBuf>,
    pub verify_data: bool,
    pub output: Option<PathBuf>,
}

pub fn resolve(assets: &Assets, overrides: RunOverrides) -> anyhow::Result<RunPlan> {
    let repo_root = resolve_repo_root(overrides.repo_root.as_deref())?;
    let manifest = assets.load_bench(&overrides.name)?;
    let suite = assets.load_suite(&manifest.bench.suite)?;
    let dataset = assets.load_dataset(&suite.suite.dataset)?;
    if !dataset.dataset.formats.contains(&manifest.dataset.format) {
        bail!(
            "benchmark `{}` requests unsupported {:?} data",
            manifest.bench.name,
            manifest.dataset.format
        );
    }

    let environment_data_root = env::var_os("SIRIUS_RUNNER_DATA_ROOT").map(PathBuf::from);
    let data_root = resolve_under_repo(
        &repo_root,
        overrides
            .data_root
            .as_deref()
            .or(environment_data_root.as_deref())
            .unwrap_or(Path::new("test_datasets")),
    )?;
    let queries = resolve_queries(
        assets,
        &suite.suite.name,
        &suite.queries,
        suite.validation.ordered_results,
        &overrides.queries,
    )?;
    let engines = expand_engines(overrides.engine.unwrap_or(manifest.engine.engine));
    let needs_sirius = engines.contains(&Engine::Sirius);
    if !needs_sirius && overrides.pin != PinPolicy::None {
        bail!("--pin applies only to Sirius runs; remove it or select --engine sirius/both");
    }
    if !needs_sirius && overrides.config.is_some() {
        bail!("--config applies only to Sirius runs; remove it or select --engine sirius/both");
    }
    if !needs_sirius && (overrides.preset.is_some() || overrides.build_dir.is_some()) {
        bail!(
            "--preset and --build-dir apply only to Sirius runs; remove them or select --engine sirius/both"
        );
    }
    let (build, build_source) = resolve_build(
        &repo_root,
        overrides.preset,
        overrides.build_dir.as_deref(),
        needs_sirius,
    )?;
    let (config, config_source) = if needs_sirius {
        resolve_config(&repo_root, overrides.config.as_deref(), &manifest)?
    } else {
        (None, "not required for DuckDB-only execution".to_string())
    };
    let dataset = resolve_dataset(
        &repo_root,
        &data_root,
        &suite.suite.dataset,
        &dataset.dataset.generator,
        &manifest,
        overrides.data.as_deref(),
        overrides.verify_data,
    )?;
    let output_dir = resolve_output_dir(
        &repo_root,
        &manifest.bench.name,
        overrides.output.as_deref(),
    )?;

    let iterations = overrides
        .iterations
        .unwrap_or(manifest.execution.iterations);
    if iterations == 0 {
        bail!("iterations must be positive");
    }
    let timeout_s = manifest.execution.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S);
    if timeout_s == 0 {
        bail!("query timeout must be positive");
    }

    let mut sources = BTreeMap::from([
        (
            "repo_root".to_string(),
            if overrides.repo_root.is_some() {
                "cli".to_string()
            } else if env::var_os("SIRIUS_REPO_ROOT").is_some() {
                "environment".to_string()
            } else {
                "repository discovery".to_string()
            },
        ),
        (
            "data_root".to_string(),
            if overrides.data_root.is_some() {
                "cli".to_string()
            } else if environment_data_root.is_some() {
                "environment".to_string()
            } else {
                "repository default".to_string()
            },
        ),
        ("build".to_string(), build_source),
        ("config".to_string(), config_source),
        (
            "iterations".to_string(),
            if overrides.iterations.is_some() {
                "cli".to_string()
            } else {
                "benchmark".to_string()
            },
        ),
        (
            "engine".to_string(),
            if overrides.engine.is_some() {
                "cli".to_string()
            } else {
                "benchmark".to_string()
            },
        ),
        (
            "data_verification".to_string(),
            if overrides.verify_data {
                "full content hashes".to_string()
            } else {
                "size and modification time".to_string()
            },
        ),
    ]);
    sources.insert(
        "dataset".to_string(),
        if overrides.data.is_some() {
            "cli".to_string()
        } else {
            "managed cache".to_string()
        },
    );

    Ok(RunPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        runner_version: env!("CARGO_PKG_VERSION").to_string(),
        runner_sha256: hashing::file(
            &env::current_exe().context("locating the running sirius-runner binary")?,
        )
        .context("hashing the running sirius-runner binary")?,
        origin: ExecutionOrigin::Local,
        repository: RepositoryIdentity {
            git_commit: git_capture(&repo_root, &["rev-parse", "HEAD"]),
            git_dirty: git_dirty(&repo_root),
        },
        benchmark: BenchmarkPlan {
            name: manifest.bench.name,
            description: manifest.bench.description,
            suite: suite.suite.name,
        },
        repo_root,
        data_root,
        output_dir,
        dataset,
        build,
        queries,
        engines,
        execution: ExecutionPlan {
            warmups: manifest.execution.warmups,
            iterations,
            timeout_s,
            validate: manifest.execution.validate,
            cache_state: "warm-up conditioned; OS page cache is uncontrolled".to_string(),
            timing_boundary: "query execution through full result materialization".to_string(),
            trial_order:
                "query-major; engine order alternates per query to reduce systematic drift"
                    .to_string(),
        },
        config,
        pin: overrides.pin,
        sources,
    })
}

fn git_capture(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_dirty(repo_root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(!output.stdout.is_empty())
}

pub fn resolve_repo_root(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    let explicit_or_env = explicit
        .map(Path::to_path_buf)
        .or_else(|| env::var_os("SIRIUS_REPO_ROOT").map(PathBuf::from));
    if let Some(path) = explicit_or_env {
        let path = fs::canonicalize(&path)
            .with_context(|| format!("resolving repository root {}", path.display()))?;
        validate_repo_root(&path)?;
        return Ok(path);
    }

    let cwd = env::current_dir().context("reading current directory")?;
    for ancestor in cwd.ancestors() {
        if is_repo_root(ancestor) {
            return fs::canonicalize(ancestor).context("resolving discovered repository root");
        }
    }
    bail!(
        "could not find the Sirius repository from {}; pass --repo-root",
        cwd.display()
    )
}

fn validate_repo_root(path: &Path) -> anyhow::Result<()> {
    if !is_repo_root(path) {
        bail!(
            "{} is not a Sirius checkout (expected pixi.toml, CMakeLists.txt, src/, and rust/Cargo.toml)",
            path.display()
        );
    }
    Ok(())
}

fn is_repo_root(path: &Path) -> bool {
    path.join("pixi.toml").is_file()
        && path.join("CMakeLists.txt").is_file()
        && path.join("src").is_dir()
        && path.join("rust/Cargo.toml").is_file()
}

fn resolve_under_repo(repo_root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    normalize_path(&resolved)
}

fn normalize_path(path: &Path) -> anyhow::Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path)
            .with_context(|| format!("resolving path {}", path.display()));
    }
    let parent = path.parent().context("path has no parent")?;
    let name = path.file_name().context("path has no file name")?;
    let parent = if parent.exists() {
        fs::canonicalize(parent)
            .with_context(|| format!("resolving parent {}", parent.display()))?
    } else {
        normalize_missing(parent)?
    };
    Ok(parent.join(name))
}

fn normalize_missing(path: &Path) -> anyhow::Result<PathBuf> {
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        suffix.push(
            existing
                .file_name()
                .context("path has no existing ancestor")?
                .to_os_string(),
        );
        existing = existing.parent().context("path has no existing ancestor")?;
    }
    let mut normalized = fs::canonicalize(existing)
        .with_context(|| format!("resolving ancestor {}", existing.display()))?;
    for component in suffix.into_iter().rev() {
        normalized.push(component);
    }
    Ok(lexically_normalize(&normalized))
}

fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn resolve_queries(
    assets: &Assets,
    suite_name: &str,
    available: &[crate::assets::QuerySpec],
    ordered_by_default: bool,
    requested: &[String],
) -> anyhow::Result<Vec<ResolvedQuery>> {
    let selected = parse_query_selection(requested, available)?;
    available
        .iter()
        .filter(|query| selected.contains(query.name.as_str()))
        .map(|query| {
            let sql = assets.read_suite_file(suite_name, &query.sql)?;
            let validation = query.validation.as_ref();
            Ok(ResolvedQuery {
                name: query.name.clone(),
                sql_sha256: hashing::bytes(sql.as_bytes()),
                sql,
                validation: crate::model::QueryValidationPlan {
                    compare: validation
                        .map(|validation| validation.compare)
                        .unwrap_or_default(),
                    float_tolerance: validation.and_then(|validation| validation.float_tolerance),
                    ordered_results: validation
                        .and_then(|validation| validation.ordered_results)
                        .unwrap_or(ordered_by_default),
                },
            })
        })
        .collect()
}

pub(crate) fn parse_query_selection(
    requested: &[String],
    available: &[crate::assets::QuerySpec],
) -> anyhow::Result<HashSet<String>> {
    let all = available
        .iter()
        .map(|query| query.name.clone())
        .collect::<HashSet<_>>();
    if requested.is_empty() {
        return Ok(all);
    }

    let mut selected = HashSet::new();
    for item in requested {
        if let Some((start, end)) = item.split_once('-') {
            let start = query_number(start)?;
            let end = query_number(end)?;
            if start > end {
                bail!("invalid descending query range `{item}`");
            }
            for number in start..=end {
                selected.insert(format!("q{number}"));
            }
        } else {
            selected.insert(normalize_query(item)?);
        }
    }
    let mut unknown = selected.difference(&all).cloned().collect::<Vec<_>>();
    unknown.sort();
    if !unknown.is_empty() {
        bail!("unknown queries: {}", unknown.join(", "));
    }
    Ok(selected)
}

fn normalize_query(value: &str) -> anyhow::Result<String> {
    Ok(format!("q{}", query_number(value)?))
}

fn query_number(value: &str) -> anyhow::Result<u32> {
    let number = value
        .strip_prefix('q')
        .unwrap_or(value)
        .parse::<u32>()
        .with_context(|| format!("invalid query `{value}`"))?;
    if number == 0 {
        bail!("invalid query `{value}`");
    }
    Ok(number)
}

fn expand_engines(engine: Engine) -> Vec<Engine> {
    match engine {
        Engine::Both => vec![Engine::Duckdb, Engine::Sirius],
        other => vec![other],
    }
}

fn resolve_build(
    repo_root: &Path,
    preset: Option<String>,
    external: Option<&Path>,
    needs_sirius: bool,
) -> anyhow::Result<(BuildPlan, String)> {
    let (preset, build_dir, action, source) = match external {
        Some(path) => (
            None,
            resolve_under_repo(repo_root, path)?,
            BuildAction::UseExisting,
            "cli build directory".to_string(),
        ),
        None => {
            let preset = preset.unwrap_or_else(|| "release".to_string());
            if !matches!(preset.as_str(), "release" | "debug" | "relwithdebinfo") {
                bail!(
                    "unsupported build preset `{preset}`; expected release, debug, or relwithdebinfo"
                );
            }
            (
                Some(preset.clone()),
                repo_root.join("build").join(&preset),
                BuildAction::IncrementalBuild,
                "incremental repository build".to_string(),
            )
        }
    };
    let mut plan = BuildPlan {
        preset,
        duckdb_binary: build_dir.join("duckdb"),
        extension: build_dir.join("extension/sirius/sirius.duckdb_extension"),
        build_dir,
        action,
    };
    if !needs_sirius {
        plan.action = BuildAction::NotRequired;
        return Ok((plan, "not required for DuckDB-only execution".to_string()));
    }
    Ok((plan, source))
}

fn resolve_config(
    repo_root: &Path,
    cli: Option<&Path>,
    manifest: &BenchManifest,
) -> anyhow::Result<(Option<PathBuf>, String)> {
    let (path, source) = if let Some(path) = cli {
        (Some(path.to_path_buf()), "cli")
    } else if let Some(path) = manifest.engine.config.as_deref() {
        (Some(path.to_path_buf()), "benchmark")
    } else if let Some(path) = env::var_os("SIRIUS_CONFIG_FILE") {
        (Some(path.into()), "environment")
    } else if repo_root.join("sirius.yaml").is_file() {
        (Some(repo_root.join("sirius.yaml")), "repository default")
    } else if let Some(path) = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".sirius/sirius.yaml"))
        .filter(|path| path.is_file())
    {
        (Some(path), "user default")
    } else {
        (None, "built-in defaults")
    };
    let Some(path) = path else {
        return Ok((None, source.to_string()));
    };
    let path = resolve_under_repo(repo_root, &path)?;
    if !path.is_file() {
        bail!("Sirius config {} does not exist", path.display());
    }
    Ok((Some(path), source.to_string()))
}

fn resolve_dataset(
    repo_root: &Path,
    data_root: &Path,
    kind: &str,
    generator: &str,
    manifest: &BenchManifest,
    external: Option<&Path>,
    verify_content: bool,
) -> anyhow::Result<DatasetPlan> {
    let spec = DatasetSpec {
        schema_version: 1,
        kind: kind.to_string(),
        generator: generator.to_string(),
        generator_version: DATASET_FORMAT_VERSION,
        scale_factor: manifest.dataset.scale_factor,
        format: manifest.dataset.format,
    };
    if let Some(path) = external {
        let path = resolve_under_repo(repo_root, path)?;
        if !path.exists() {
            bail!("external dataset {} does not exist", path.display());
        }
        return Ok(DatasetPlan {
            id: hashing::json(&spec)?,
            spec,
            recipe: None,
            entry_dir: path.clone(),
            data_path: path,
            cache: CacheState::External,
            estimated_bytes: estimate_dataset_bytes(manifest.dataset.scale_factor),
            managed: false,
            verify_content,
        });
    }

    #[derive(serde::Serialize)]
    struct ManagedDatasetKey<'a> {
        spec: &'a DatasetSpec,
        recipe: &'a crate::model::DatasetRecipe,
    }

    let recipe = crate::dataset::managed_recipe(repo_root)?;
    let recipe_id = hashing::json(&recipe)?;
    let id = hashing::json(&ManagedDatasetKey {
        spec: &spec,
        recipe: &recipe,
    })?;
    let scale = scale_label(manifest.dataset.scale_factor);
    let format = match manifest.dataset.format {
        DataFormat::Parquet => "parquet",
        DataFormat::Duckdb => "duckdb",
    };
    let entry_dir = data_root
        .join(".sirius/datasets")
        .join(kind)
        .join(format!("v{}", DATASET_FORMAT_VERSION))
        .join(format!("sf{scale}"))
        .join(format)
        .join(format!("recipe-{recipe_id}"));
    let data_path = entry_dir.join("data");
    let mut plan = DatasetPlan {
        spec,
        recipe: Some(recipe),
        id,
        entry_dir,
        data_path,
        cache: CacheState::Unknown,
        estimated_bytes: estimate_dataset_bytes(manifest.dataset.scale_factor),
        managed: true,
        verify_content,
    };
    plan.cache = if !plan.entry_dir.exists() {
        CacheState::Miss
    } else if crate::dataset::inspect(&plan).is_ok_and(|receipt| receipt.is_some()) {
        CacheState::Hit
    } else {
        CacheState::Invalid
    };
    Ok(plan)
}

fn scale_label(scale: f64) -> String {
    if scale.fract() == 0.0 {
        format!("{scale:.0}")
    } else {
        scale.to_string().replace('.', "p")
    }
}

fn estimate_dataset_bytes(scale: f64) -> u64 {
    (scale * 1_000_000_000.0).ceil() as u64
}

pub(crate) fn resolve_output_dir(
    repo_root: &Path,
    benchmark: &str,
    explicit: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let output = match explicit {
        Some(path) => resolve_under_repo(repo_root, path)?,
        None => default_output_dir(repo_root, benchmark),
    };
    if output.exists() {
        bail!(
            "result bundle {} already exists; choose a new --output",
            output.display()
        );
    }
    Ok(output)
}

fn default_output_dir(repo_root: &Path, benchmark: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    repo_root
        .join("benchmark-runs")
        .join(format!("{timestamp}-{benchmark}-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(name: &str) -> RunOverrides {
        RunOverrides {
            name: name.to_string(),
            repo_root: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")),
            data_root: Some(PathBuf::from("test_datasets")),
            queries: Vec::new(),
            iterations: None,
            engine: None,
            config: None,
            pin: PinPolicy::None,
            preset: None,
            build_dir: None,
            data: None,
            verify_data: false,
            output: Some(PathBuf::from("benchmark-runs/test-plan-does-not-exist")),
        }
    }

    #[test]
    fn embedded_benchmark_resolves_to_one_plain_plan() {
        let plan = resolve(&Assets::Embedded, overrides("tpch-sf1")).unwrap();
        assert_eq!(plan.queries.len(), 22);
        assert_eq!(plan.engines, [Engine::Duckdb, Engine::Sirius]);
        assert_eq!(plan.execution.warmups, 1);
        assert_eq!(plan.execution.iterations, 3);
        assert_eq!(plan.build.action, BuildAction::IncrementalBuild);
        assert!(plan.repo_root.join("pixi.toml").is_file());
    }

    #[test]
    fn cache_state_does_not_trust_a_receipt_filename_alone() {
        let temp = tempfile::tempdir().unwrap();
        let mut input = overrides("tpch-sf1");
        input.data_root = Some(temp.path().to_path_buf());
        input.output = Some(temp.path().join("first-output"));
        let unresolved = resolve(&Assets::Embedded, input).unwrap();
        let entry = unresolved.dataset.entry_dir;
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("receipt.json"), "{}").unwrap();
        let mut input = overrides("tpch-sf1");
        input.data_root = Some(temp.path().to_path_buf());
        input.output = Some(temp.path().join("output"));

        let plan = resolve(&Assets::Embedded, input).unwrap();

        assert_eq!(plan.dataset.cache, CacheState::Invalid);
    }

    #[test]
    fn managed_recipe_changes_select_a_new_cache_entry() {
        let temp = tempfile::tempdir().unwrap();
        let recipe_dir = temp.path().join("test/tpch_performance");
        fs::create_dir_all(&recipe_dir).unwrap();
        fs::write(
            recipe_dir.join("tpchgen-revision.txt"),
            format!("{}\n", "a".repeat(40)),
        )
        .unwrap();
        let wrapper = recipe_dir.join("generate_tpch_data.sh");
        fs::write(&wrapper, "#!/usr/bin/env bash\n# first recipe\n").unwrap();
        let benchmark = Assets::Embedded.load_bench("tpch-sf1").unwrap();
        let data_root = temp.path().join("data");

        let first = resolve_dataset(
            temp.path(),
            &data_root,
            "tpch",
            "tpchgen",
            &benchmark,
            None,
            false,
        )
        .unwrap();
        fs::write(&wrapper, "#!/usr/bin/env bash\n# second recipe\n").unwrap();
        let second = resolve_dataset(
            temp.path(),
            &data_root,
            "tpch",
            "tpchgen",
            &benchmark,
            None,
            false,
        )
        .unwrap();

        assert_ne!(first.recipe, second.recipe);
        assert_ne!(first.id, second.id);
        assert_ne!(first.entry_dir, second.entry_dir);
    }

    #[test]
    fn query_ranges_are_normalized_and_suite_order_is_preserved() {
        let mut input = overrides("tpch-sf1");
        input.queries = vec!["q6-q8".to_string(), "1".to_string()];
        let plan = resolve(&Assets::Embedded, input).unwrap();
        let names = plan
            .queries
            .iter()
            .map(|query| query.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["q1", "q6", "q7", "q8"]);
    }

    #[test]
    fn invalid_query_and_preset_are_rejected() {
        let mut input = overrides("tpch-sf1");
        input.queries = vec!["q99".to_string()];
        assert!(resolve(&Assets::Embedded, input).is_err());

        let mut input = overrides("tpch-sf1");
        input.preset = Some("surprise".to_string());
        assert!(resolve(&Assets::Embedded, input).is_err());
    }

    #[test]
    fn repository_discovery_requires_strong_markers() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("pixi.toml"), "").unwrap();
        assert!(validate_repo_root(temp.path()).is_err());
    }
}
