use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    assets::DataFormat,
    cancel, hashing,
    model::{DatasetPlan, DatasetRecipe, DatasetSpec},
    process,
    progress::{Reporter, Stage},
};

const RECEIPT_SCHEMA_VERSION: u32 = 2;
const RECIPE_SCHEMA_VERSION: u32 = 1;
const MANAGED_TPCH_JOBS: u32 = 8;
const TPCHGEN_REVISION_FILE: &str = "test/tpch_performance/tpchgen-revision.txt";
const TPCH_GENERATOR: &str = "test/tpch_performance/generate_tpch_data.sh";
const TPCHGEN_EXECUTABLE: &str = "test_datasets/tpchgen-rs/target/release/tpchgen-cli";
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(250);
const LOCK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const TPCH_TABLES: [&str; 8] = [
    "customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetReceipt {
    pub schema_version: u32,
    pub cache_key: String,
    pub dataset_id: String,
    pub spec: DatasetSpec,
    pub recipe: DatasetRecipe,
    pub producer: DatasetProducer,
    pub created_unix_s: u64,
    pub files: Vec<DatasetFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetProducer {
    pub executable_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified_ns: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedDataset {
    pub path: PathBuf,
    pub identity: String,
    pub stability_id: String,
    pub receipt: Option<DatasetReceipt>,
    pub cache_hit: bool,
}

pub fn managed_recipe(repo_root: &Path) -> anyhow::Result<DatasetRecipe> {
    let wrapper = repo_root.join(TPCH_GENERATOR);
    ensure!(
        wrapper.is_file(),
        "dataset generator {} is missing",
        wrapper.display()
    );
    let revision_path = repo_root.join(TPCHGEN_REVISION_FILE);
    let revision = fs::read_to_string(&revision_path).with_context(|| {
        format!(
            "reading pinned tpchgen revision {}",
            revision_path.display()
        )
    })?;
    let tpchgen_revision = revision.trim();
    ensure!(
        tpchgen_revision.len() == 40
            && tpchgen_revision
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "pinned tpchgen revision in {} must be 40 lowercase hexadecimal characters",
        revision_path.display()
    );
    Ok(DatasetRecipe {
        schema_version: RECIPE_SCHEMA_VERSION,
        wrapper_sha256: hashing::file(&wrapper)?,
        tpchgen_revision: tpchgen_revision.to_string(),
        jobs: MANAGED_TPCH_JOBS,
    })
}

pub fn verify_stable(
    plan: &DatasetPlan,
    prepared: &PreparedDataset,
    phase: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    reporter.status(&format!("Verifying dataset stability {phase}"))?;
    let current = stability_id(plan)?;
    ensure!(
        current == prepared.stability_id,
        "dataset {} changed {phase}; refusing to publish or record results for a stale identity",
        prepared.path.display()
    );
    reporter.detail(&format!(
        "Dataset remained stable {phase} ({})",
        short_id(&current)
    ))?;
    Ok(())
}

pub fn prepare(
    plan: &DatasetPlan,
    repo_root: &Path,
    data_root: &Path,
    reporter: &mut impl Reporter,
) -> anyhow::Result<PreparedDataset> {
    if !plan.managed {
        reporter.status(&format!(
            "Validating external dataset {}",
            plan.data_path.display()
        ))?;
        let (identity, stability_id) = external_identity(plan, reporter)?;
        reporter.status(&format!("Using external dataset ({})", short_id(&identity)))?;
        return Ok(PreparedDataset {
            path: plan.data_path.clone(),
            identity,
            stability_id,
            receipt: None,
            cache_hit: false,
        });
    }
    if plan.spec.generator != "tpchgen" {
        bail!(
            "managed dataset generator `{}` is not supported",
            plan.spec.generator
        );
    }
    let recipe = plan
        .recipe
        .as_ref()
        .context("managed dataset plan has no generator recipe")?;

    let lock_path = data_root
        .join(".sirius/locks/datasets")
        .join(format!("{}.lock", plan.id));
    let lock_parent = lock_path.parent().context("dataset lock has no parent")?;
    fs::create_dir_all(lock_parent)
        .with_context(|| format!("creating {}", lock_parent.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    acquire_lock(&lock, &plan.id, reporter)?;

    let cache_parent = plan
        .entry_dir
        .parent()
        .context("dataset cache entry has no parent")?;
    scavenge_temporary_entries(cache_parent, &plan.id, reporter)?;
    if plan.entry_dir.exists() {
        let cached = (|| {
            let receipt = load_and_validate(plan)?;
            if plan.verify_content {
                reporter
                    .status("Full dataset verification requested; checking cached file hashes")?;
                let current = checksum_inventory(&plan.data_path, reporter)?;
                ensure!(
                    same_content(&current, &receipt.files),
                    "dataset cache entry {} failed full content verification",
                    plan.entry_dir.display()
                );
                reporter.status("Dataset content hashes match the immutable receipt")?;
            } else {
                reporter.status(
                    "Dataset cache receipt matches file sizes and modification times; \
                     pass --verify-data to rehash contents",
                )?;
            }
            Ok::<_, anyhow::Error>(receipt)
        })();
        match cached {
            Ok(receipt) => {
                reporter.status(&format!(
                    "Dataset cache hit: {} ({})",
                    plan.data_path.display(),
                    short_id(&receipt.dataset_id)
                ))?;
                return Ok(PreparedDataset {
                    path: plan.data_path.clone(),
                    identity: receipt.dataset_id.clone(),
                    stability_id: stability_id(plan)?,
                    receipt: Some(receipt),
                    cache_hit: true,
                });
            }
            Err(error) => discard_invalid_entry(plan, &error, reporter)?,
        }
    }

    if plan.spec.format != DataFormat::Parquet {
        bail!(
            "managed {:?} generation is not implemented; pass --data with an existing dataset",
            plan.spec.format
        );
    }
    reporter.status(&format!(
        "Dataset cache miss: TPC-H SF{} parquet",
        plan.spec.scale_factor
    ))?;
    reporter.status(&format!(
        "Estimated output size: {}",
        human_bytes(plan.estimated_bytes)
    ))?;
    check_free_space(&plan.entry_dir, plan.estimated_bytes, reporter)?;

    let parent = cache_parent;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let temp = parent.join(format!(
        ".tmp-{}-{}-{}",
        plan.id,
        std::process::id(),
        unix_nanos()
    ));
    fs::create_dir(&temp).with_context(|| format!("creating {}", temp.display()))?;
    let mut cleanup = TempDirGuard(Some(temp.clone()));
    let output = temp.join("data");
    let generator = repo_root.join(TPCH_GENERATOR);
    if !generator.is_file() {
        bail!("dataset generator {} is missing", generator.display());
    }

    let mut command = Command::new("bash");
    command
        .current_dir(repo_root)
        .arg(&generator)
        .arg(plan.spec.scale_factor.to_string())
        .arg("--format")
        .arg("parquet")
        .arg("--output")
        .arg(&output)
        .arg("--jobs")
        .arg(recipe.jobs.to_string())
        .stdin(Stdio::null());
    process::run(
        &mut command,
        format!("Generating dataset at {}", output.display()),
        reporter,
    )?;
    validate_data_path(&output, DataFormat::Parquet)?;

    reporter.status("Checksumming the generated dataset for its immutable receipt")?;
    let files = checksum_inventory(&output, reporter)?;
    verify_recipe_source(repo_root, recipe)?;
    let executable = repo_root.join(TPCHGEN_EXECUTABLE);
    ensure!(
        executable.is_file(),
        "tpchgen executable {} is missing after generation",
        executable.display()
    );
    let producer = DatasetProducer {
        executable_sha256: hashing::file_with_progress(
            &executable,
            "exact tpchgen executable",
            reporter,
        )?,
    };
    let dataset_id = receipt_identity(&plan.spec, recipe, &producer, &files)?;
    let receipt = DatasetReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        cache_key: plan.id.clone(),
        dataset_id,
        spec: plan.spec.clone(),
        recipe: recipe.clone(),
        producer,
        created_unix_s: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        files,
    };
    write_receipt(&temp.join("receipt.json"), &receipt)?;
    fs::rename(&temp, &plan.entry_dir).with_context(|| {
        format!(
            "publishing dataset {} to {}",
            temp.display(),
            plan.entry_dir.display()
        )
    })?;
    cleanup.0 = None;
    reporter.status(&format!(
        "Published dataset cache entry {}",
        plan.entry_dir.display()
    ))?;

    Ok(PreparedDataset {
        path: plan.data_path.clone(),
        identity: receipt.dataset_id.clone(),
        stability_id: stability_id(plan)?,
        receipt: Some(receipt),
        cache_hit: false,
    })
}

fn discard_invalid_entry(
    plan: &DatasetPlan,
    error: &anyhow::Error,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    let parent = plan
        .entry_dir
        .parent()
        .context("dataset cache entry has no parent")?;
    let quarantine = parent.join(format!(
        ".invalid-{}-{}-{}",
        plan.id,
        std::process::id(),
        unix_nanos()
    ));
    reporter.status(&format!(
        "Dataset cache entry is invalid and will be regenerated: {error:#}"
    ))?;
    fs::rename(&plan.entry_dir, &quarantine).with_context(|| {
        format!(
            "quarantining invalid dataset {} as {}",
            plan.entry_dir.display(),
            quarantine.display()
        )
    })?;
    fs::remove_dir_all(&quarantine)
        .with_context(|| format!("removing invalid dataset {}", quarantine.display()))
}

fn scavenge_temporary_entries(
    parent: &Path,
    dataset_id: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    if !parent.is_dir() {
        return Ok(());
    }
    let prefix = format!(".tmp-{dataset_id}-");
    for entry in fs::read_dir(parent).with_context(|| format!("reading {}", parent.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let file_type = entry.file_type()?;
        ensure!(
            file_type.is_dir() && !file_type.is_symlink(),
            "refusing to remove unexpected dataset temporary entry {}",
            entry.path().display()
        );
        reporter.status(&format!(
            "Removing an incomplete dataset staging directory {}",
            entry.path().display()
        ))?;
        fs::remove_dir_all(entry.path())?;
    }
    Ok(())
}

pub fn inspect(plan: &DatasetPlan) -> anyhow::Result<Option<DatasetReceipt>> {
    if !plan.managed || !plan.entry_dir.exists() {
        return Ok(None);
    }
    load_and_validate(plan).map(Some)
}

fn acquire_lock(file: &File, id: &str, reporter: &mut impl Reporter) -> anyhow::Result<()> {
    let mut waiting: Option<Stage> = None;
    loop {
        cancel::check()?;
        match file.try_lock() {
            Ok(()) => {
                if let Some(stage) = waiting {
                    stage.complete(reporter)?;
                }
                return Ok(());
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                if waiting.is_none() {
                    waiting = Some(Stage::start(
                        reporter,
                        format!("Waiting for dataset cache lock {}", short_id(id)),
                    )?);
                }
                thread::sleep(LOCK_POLL_INTERVAL);
                if let Some(stage) = &mut waiting {
                    stage.heartbeat(reporter, LOCK_HEARTBEAT_INTERVAL)?;
                }
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error).context("locking dataset cache entry");
            }
        }
    }
}

fn load_and_validate(plan: &DatasetPlan) -> anyhow::Result<DatasetReceipt> {
    let receipt_path = plan.entry_dir.join("receipt.json");
    let input = File::open(&receipt_path).with_context(|| {
        format!(
            "dataset cache entry {} is incomplete: missing receipt.json",
            plan.entry_dir.display()
        )
    })?;
    let receipt: DatasetReceipt = serde_json::from_reader(input)
        .with_context(|| format!("reading {}", receipt_path.display()))?;
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION
        || receipt.cache_key != plan.id
        || receipt.spec != plan.spec
        || Some(&receipt.recipe) != plan.recipe.as_ref()
    {
        bail!(
            "dataset cache receipt {} does not match the requested dataset; move it aside or use --data",
            receipt_path.display()
        );
    }
    let current = validated_inventory(&plan.data_path, plan.spec.format)?;
    if !same_inventory(&current, &receipt.files) {
        bail!(
            "dataset cache entry {} no longer matches its file inventory; move it aside and rerun",
            plan.entry_dir.display()
        );
    }
    if receipt.dataset_id
        != receipt_identity(
            &receipt.spec,
            &receipt.recipe,
            &receipt.producer,
            &receipt.files,
        )?
    {
        bail!(
            "dataset cache receipt {} has an invalid identity",
            receipt_path.display()
        );
    }
    Ok(receipt)
}

fn receipt_identity(
    spec: &DatasetSpec,
    recipe: &DatasetRecipe,
    producer: &DatasetProducer,
    files: &[DatasetFile],
) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct ContentFile<'a> {
        path: &'a Path,
        size: u64,
        sha256: &'a Option<String>,
    }

    #[derive(Serialize)]
    struct Identity<'a> {
        schema_version: u32,
        spec: &'a DatasetSpec,
        recipe: &'a DatasetRecipe,
        producer: &'a DatasetProducer,
        files: Vec<ContentFile<'a>>,
    }
    hashing::json(&Identity {
        schema_version: RECEIPT_SCHEMA_VERSION,
        spec,
        recipe,
        producer,
        files: files
            .iter()
            .map(|file| ContentFile {
                path: &file.path,
                size: file.size,
                sha256: &file.sha256,
            })
            .collect(),
    })
}

fn external_identity(
    plan: &DatasetPlan,
    reporter: &mut impl Reporter,
) -> anyhow::Result<(String, String)> {
    let inventory = validated_inventory(&plan.data_path, plan.spec.format)?;
    let canonical_path = fs::canonicalize(&plan.data_path)?;
    let files = if plan.verify_content {
        reporter.status("Full dataset verification requested; hashing every external file")?;
        checksum_inventory(&plan.data_path, reporter)?
    } else {
        reporter.status(
            "Using fast external-data identity (path, size, and modification time); \
             pass --verify-data to hash contents",
        )?;
        inventory
    };
    let identity = if plan.verify_content {
        verified_external_identity(&plan.spec, &files)?
    } else {
        hashing::json(&ExternalIdentity {
            schema_version: 1,
            canonical_path: canonical_path.clone(),
            spec: &plan.spec,
            files: &files,
        })?
    };
    let stability_id = inventory_identity(&canonical_path, &plan.spec, &files)?;
    Ok((identity, stability_id))
}

fn verified_external_identity(spec: &DatasetSpec, files: &[DatasetFile]) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct ContentFile<'a> {
        path: String,
        size: u64,
        sha256: &'a str,
    }

    #[derive(Serialize)]
    struct ContentIdentity<'a> {
        schema_version: u32,
        spec: &'a DatasetSpec,
        files: Vec<ContentFile<'a>>,
    }

    let mut files = files
        .iter()
        .map(|file| {
            let sha256 = file
                .sha256
                .as_deref()
                .context("verified external dataset file has no content hash")?;
            ensure!(
                sha256.len() == 64
                    && sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "verified external dataset file {} has an invalid SHA-256 digest",
                file.path.display()
            );
            Ok(ContentFile {
                path: normalized_relative_path(&file.path)?,
                size: file.size,
                sha256,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));

    hashing::json(&ContentIdentity {
        schema_version: 1,
        spec,
        files,
    })
}

fn normalized_relative_path(path: &Path) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .with_context(|| format!("dataset path {} is not valid UTF-8", path.display()))?
                    .to_owned(),
            ),
            _ => bail!(
                "dataset inventory path {} is not a normalized relative path",
                path.display()
            ),
        }
    }
    ensure!(
        !parts.is_empty(),
        "dataset inventory contains an empty relative path"
    );
    Ok(parts.join("/"))
}

fn stability_id(plan: &DatasetPlan) -> anyhow::Result<String> {
    let files = validated_inventory(&plan.data_path, plan.spec.format)?;
    inventory_identity(&fs::canonicalize(&plan.data_path)?, &plan.spec, &files)
}

fn inventory_identity(
    canonical_path: &Path,
    spec: &DatasetSpec,
    files: &[DatasetFile],
) -> anyhow::Result<String> {
    #[derive(Serialize)]
    struct StabilityFile<'a> {
        path: &'a Path,
        size: u64,
        modified_ns: u128,
    }

    #[derive(Serialize)]
    struct StabilityIdentity<'a> {
        schema_version: u32,
        canonical_path: &'a Path,
        spec: &'a DatasetSpec,
        files: Vec<StabilityFile<'a>>,
    }

    hashing::json(&StabilityIdentity {
        schema_version: 1,
        canonical_path,
        spec,
        files: files
            .iter()
            .map(|file| StabilityFile {
                path: &file.path,
                size: file.size,
                modified_ns: file.modified_ns,
            })
            .collect(),
    })
}

fn verify_recipe_source(repo_root: &Path, recipe: &DatasetRecipe) -> anyhow::Result<()> {
    let current = managed_recipe(repo_root)?;
    ensure!(
        current == *recipe,
        "managed dataset recipe changed during generation"
    );
    let checkout = repo_root.join("test_datasets/tpchgen-rs");
    let revision = git_revision(&checkout).with_context(|| {
        format!(
            "reading tpchgen revision from generated checkout {}",
            checkout.display()
        )
    })?;
    ensure!(
        revision == recipe.tpchgen_revision,
        "tpchgen checkout is at {revision}, expected {}",
        recipe.tpchgen_revision
    );
    ensure!(
        git_is_clean(&checkout)?,
        "tpchgen checkout {} changed during generation",
        checkout.display()
    );
    Ok(())
}

fn validate_data_path(path: &Path, format: DataFormat) -> anyhow::Result<()> {
    validated_inventory(path, format).map(|_| ())
}

fn validated_inventory(path: &Path, format: DataFormat) -> anyhow::Result<Vec<DatasetFile>> {
    match format {
        DataFormat::Parquet => {
            if !path.is_dir() {
                bail!("parquet dataset {} is not a directory", path.display());
            }
            let files = inventory(path)?;
            for table in TPCH_TABLES {
                let found = files
                    .iter()
                    .any(|file| matches_parquet_table(&file.path, table));
                if !found {
                    bail!(
                        "parquet dataset {} has no files for TPC-H table `{table}`",
                        path.display()
                    );
                }
            }
            Ok(files)
        }
        DataFormat::Duckdb if !path.is_file() => {
            bail!("DuckDB dataset {} is not a file", path.display());
        }
        DataFormat::Duckdb => inventory(path),
    }
}

fn matches_parquet_table(path: &Path, table: &str) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let top_level = path.components().count() == 1
        && (name == format!("{table}.parquet")
            || (name.starts_with(&format!("{table}_")) && name.ends_with(".parquet")));
    let table_directory = path.components().count() == 2
        && path
            .parent()
            .is_some_and(|parent| parent == Path::new(table))
        && name.ends_with(".parquet");
    top_level || table_directory
}

fn inventory(path: &Path) -> anyhow::Result<Vec<DatasetFile>> {
    if path.is_file() {
        return Ok(vec![DatasetFile {
            path: path.file_name().context("dataset file has no name")?.into(),
            size: path.metadata()?.len(),
            modified_ns: modified_ns(path)?,
            sha256: None,
        }]);
    }
    let mut files = Vec::new();
    inventory_dir(path, path, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.is_empty() {
        bail!("dataset {} contains no files", path.display());
    }
    Ok(files)
}

fn inventory_dir(
    root: &Path,
    directory: &Path,
    files: &mut Vec<DatasetFile>,
) -> anyhow::Result<()> {
    for entry in
        fs::read_dir(directory).with_context(|| format!("reading {}", directory.display()))?
    {
        cancel::check()?;
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            inventory_dir(root, &path, files)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            files.push(DatasetFile {
                path: path
                    .strip_prefix(root)
                    .context("inventory path escaped dataset root")?
                    .to_path_buf(),
                size: metadata.len(),
                modified_ns: metadata
                    .modified()?
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                sha256: None,
            });
        } else {
            bail!("dataset contains unsupported entry {}", path.display());
        }
    }
    Ok(())
}

fn checksum_inventory(
    root: &Path,
    reporter: &mut impl Reporter,
) -> anyhow::Result<Vec<DatasetFile>> {
    let mut files = inventory(root)?;
    let file_count = files.len();
    for (index, file) in files.iter_mut().enumerate() {
        reporter.status(&format!(
            "Checksumming dataset file {}/{}: {}",
            index + 1,
            file_count,
            file.path.display()
        ))?;
        let actual = if root.is_file() {
            root.to_path_buf()
        } else {
            root.join(&file.path)
        };
        let sha256 = hashing::file_with_progress(&actual, "dataset file", reporter)?;
        let metadata = actual
            .metadata()
            .with_context(|| format!("restating dataset file {}", actual.display()))?;
        let current_modified_ns = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        ensure!(
            metadata.len() == file.size && current_modified_ns == file.modified_ns,
            "dataset file {} changed while it was being checksummed",
            actual.display()
        );
        file.sha256 = Some(sha256);
    }
    Ok(files)
}

fn same_inventory(current: &[DatasetFile], receipt: &[DatasetFile]) -> bool {
    current.len() == receipt.len()
        && current.iter().zip(receipt).all(|(current, receipt)| {
            current.path == receipt.path
                && current.size == receipt.size
                && current.modified_ns == receipt.modified_ns
                && receipt.sha256.as_ref().is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        })
}

fn same_content(current: &[DatasetFile], receipt: &[DatasetFile]) -> bool {
    current.len() == receipt.len()
        && current.iter().zip(receipt).all(|(current, receipt)| {
            current.path == receipt.path
                && current.size == receipt.size
                && current.sha256.is_some()
                && current.sha256 == receipt.sha256
        })
}

fn modified_ns(path: &Path) -> anyhow::Result<u128> {
    Ok(path
        .metadata()?
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos())
}

fn check_free_space(
    entry_dir: &Path,
    estimated_bytes: u64,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    let mut existing = entry_dir;
    while !existing.exists() {
        existing = existing
            .parent()
            .context("data root has no existing parent")?;
    }
    let output = Command::new("df")
        .arg("-Pk")
        .arg(existing)
        .output()
        .context("checking free disk space with df")?;
    if !output.status.success() {
        bail!(
            "df failed while checking free space for {}",
            existing.display()
        );
    }
    let text = String::from_utf8(output.stdout).context("df returned non-UTF-8 output")?;
    let line = text
        .lines()
        .last()
        .context("df returned no filesystem row")?;
    let available_kib = line
        .split_whitespace()
        .nth(3)
        .context("could not parse available space from df")?
        .parse::<u64>()
        .context("could not parse available space from df")?;
    let available = available_kib.saturating_mul(1024);
    reporter.status(&format!(
        "Free space: {} (estimated requirement {})",
        human_bytes(available),
        human_bytes(estimated_bytes)
    ))?;
    if available < estimated_bytes {
        bail!(
            "not enough free space for the dataset: {} available, approximately {} required",
            human_bytes(available),
            human_bytes(estimated_bytes)
        );
    }
    Ok(())
}

fn write_receipt(path: &Path, receipt: &DatasetReceipt) -> anyhow::Result<()> {
    let output = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    serde_json::to_writer_pretty(output, receipt)
        .with_context(|| format!("writing {}", path.display()))
}

fn git_revision(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_is_clean(repo: &Path) -> anyhow::Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain", "--untracked-files=all"])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("checking tpchgen checkout {}", repo.display()))?;
    ensure!(
        output.status.success(),
        "git status failed for tpchgen checkout {}",
        repo.display()
    );
    Ok(output.stdout.is_empty())
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(12)]
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[derive(Serialize)]
struct ExternalIdentity<'a> {
    schema_version: u32,
    canonical_path: PathBuf,
    spec: &'a DatasetSpec,
    files: &'a [DatasetFile],
}

struct TempDirGuard(Option<PathBuf>);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn external_plan(path: &Path) -> DatasetPlan {
        DatasetPlan {
            spec: DatasetSpec {
                schema_version: 1,
                kind: "tpch".into(),
                generator: "tpchgen".into(),
                generator_version: 1,
                scale_factor: 1.0,
                format: DataFormat::Parquet,
            },
            recipe: None,
            id: "external".into(),
            entry_dir: path.to_path_buf(),
            data_path: path.to_path_buf(),
            cache: crate::model::CacheState::External,
            estimated_bytes: 1,
            managed: false,
            verify_content: false,
        }
    }

    fn create_external_tpch(path: &Path) {
        fs::create_dir_all(path).unwrap();
        for table in TPCH_TABLES {
            fs::write(path.join(format!("{table}.parquet")), table).unwrap();
        }
    }

    #[test]
    fn inventory_is_sorted_and_detects_changes() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("z"), "123").unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        fs::write(temp.path().join("nested/a"), "12").unwrap();
        let files = inventory(temp.path()).unwrap();
        assert_eq!(files[0].path, Path::new("nested/a"));
        assert_eq!(files[1].path, Path::new("z"));
        assert_eq!(files[1].size, 3);
    }

    #[test]
    fn managed_inventory_detects_same_size_mutations_without_rehashing_hits() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("data");
        fs::write(&path, "one").unwrap();
        File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
            .unwrap();
        let mut receipt = inventory(temp.path()).unwrap();
        receipt[0].sha256 = Some(hashing::file(&path).unwrap());

        fs::write(&path, "two").unwrap();
        File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(2)))
            .unwrap();
        let current = inventory(temp.path()).unwrap();

        assert_eq!(receipt[0].size, current[0].size);
        assert!(!same_inventory(&current, &receipt));
    }

    #[test]
    fn full_verification_detects_same_size_same_mtime_mutations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("data");
        let timestamp = UNIX_EPOCH + Duration::from_secs(1);
        fs::write(&path, "one").unwrap();
        File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        let mut receipt = inventory(temp.path()).unwrap();
        receipt[0].sha256 = Some(hashing::file(&path).unwrap());

        fs::write(&path, "two").unwrap();
        File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        let current = inventory(temp.path()).unwrap();
        assert!(same_inventory(&current, &receipt));

        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);
        let checksummed = checksum_inventory(temp.path(), &mut progress).unwrap();
        assert!(!same_content(&checksummed, &receipt));
    }

    #[test]
    fn prepared_external_dataset_detects_mutation_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        create_external_tpch(&data);
        let plan = external_plan(&data);
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);
        let prepared = prepare(&plan, temp.path(), temp.path(), &mut progress).unwrap();

        fs::write(data.join("lineitem.parquet"), "changed").unwrap();
        let error = verify_stable(
            &plan,
            &prepared,
            "after reference generation",
            &mut progress,
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed after reference generation")
        );
    }

    #[test]
    fn runner_owned_invalid_and_incomplete_entries_are_scoped_and_removed() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let entry = cache.join("dataset-key");
        fs::create_dir_all(entry.join("data")).unwrap();
        fs::write(entry.join("data/partial"), "bad").unwrap();
        fs::create_dir_all(cache.join(".tmp-dataset-key-old")).unwrap();
        fs::create_dir_all(cache.join(".tmp-other-key-old")).unwrap();
        let mut plan = external_plan(entry.join("data").as_path());
        plan.id = "dataset-key".to_owned();
        plan.entry_dir = entry.clone();
        plan.managed = true;
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);

        discard_invalid_entry(&plan, &anyhow::anyhow!("corrupt"), &mut progress).unwrap();
        scavenge_temporary_entries(&cache, &plan.id, &mut progress).unwrap();

        assert!(!entry.exists());
        assert!(!cache.join(".tmp-dataset-key-old").exists());
        assert!(cache.join(".tmp-other-key-old").is_dir());
    }

    #[test]
    fn verified_external_identity_is_stable_across_dataset_roots() {
        let temp = tempfile::tempdir().unwrap();
        let first_data = temp.path().join("first/data");
        let second_data = temp.path().join("second/data");
        create_external_tpch(&first_data);
        create_external_tpch(&second_data);
        let mut first_plan = external_plan(&first_data);
        first_plan.verify_content = true;
        let mut second_plan = external_plan(&second_data);
        second_plan.verify_content = true;
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);

        let (first_identity, first_stability) =
            external_identity(&first_plan, &mut progress).unwrap();
        let (second_identity, second_stability) =
            external_identity(&second_plan, &mut progress).unwrap();

        assert_eq!(first_identity, second_identity);
        assert_ne!(first_stability, second_stability);
    }

    #[test]
    fn verified_external_identity_ignores_touch_but_stability_check_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        create_external_tpch(&data);
        let mut plan = external_plan(&data);
        plan.verify_content = true;
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);
        let prepared = prepare(&plan, temp.path(), temp.path(), &mut progress).unwrap();

        let touched = data.join("lineitem.parquet");
        File::open(&touched)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
            .unwrap();
        let (touched_identity, touched_stability) =
            external_identity(&plan, &mut progress).unwrap();

        assert_eq!(prepared.identity, touched_identity);
        assert_ne!(prepared.stability_id, touched_stability);
        let error =
            verify_stable(&plan, &prepared, "after a metadata change", &mut progress).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("changed after a metadata change")
        );
    }

    #[test]
    fn exact_generator_executable_is_part_of_the_dataset_identity() {
        let spec = DatasetSpec {
            schema_version: 1,
            kind: "tpch".into(),
            generator: "tpchgen".into(),
            generator_version: 1,
            scale_factor: 1.0,
            format: DataFormat::Parquet,
        };
        let recipe = DatasetRecipe {
            schema_version: 1,
            wrapper_sha256: "a".repeat(64),
            tpchgen_revision: "b".repeat(40),
            jobs: 8,
        };
        let files = vec![DatasetFile {
            path: PathBuf::from("lineitem.parquet"),
            size: 1,
            modified_ns: 1,
            sha256: Some("c".repeat(64)),
        }];
        let first = receipt_identity(
            &spec,
            &recipe,
            &DatasetProducer {
                executable_sha256: "d".repeat(64),
            },
            &files,
        )
        .unwrap();
        let second = receipt_identity(
            &spec,
            &recipe,
            &DatasetProducer {
                executable_sha256: "e".repeat(64),
            },
            &files,
        )
        .unwrap();

        assert_ne!(first, second);
    }

    #[test]
    fn byte_format_is_readable() {
        assert_eq!(human_bytes(500), "500.0 B");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn parquet_table_matching_rejects_similarly_named_non_parquet_files() {
        assert!(matches_parquet_table(
            Path::new("lineitem.parquet"),
            "lineitem"
        ));
        assert!(matches_parquet_table(
            Path::new("lineitem/chunk.parquet"),
            "lineitem"
        ));
        assert!(!matches_parquet_table(
            Path::new("lineitem_notes.txt"),
            "lineitem"
        ));
        assert!(!matches_parquet_table(
            Path::new("archive/lineitem.parquet"),
            "lineitem"
        ));
    }
}
