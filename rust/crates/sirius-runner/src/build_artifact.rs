use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    hashing,
    model::{ArtifactIdentity, BuildAction, BuildPlan},
    process,
    progress::Reporter,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedBuild {
    pub build_dir: PathBuf,
    pub duckdb_binary: PathBuf,
    pub extension: PathBuf,
    pub identity: ArtifactIdentity,
    pub provenance: BuildProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildProvenance {
    pub source: String,
    pub git_commit: Option<String>,
    pub git_dirty: Option<bool>,
    pub preset: Option<String>,
    pub incremental_build_invoked: bool,
    pub pixi_pack_key: Option<String>,
}

pub fn prepare(
    plan: &BuildPlan,
    repo_root: &Path,
    config: Option<&Path>,
    reporter: &mut impl Reporter,
) -> anyhow::Result<PreparedBuild> {
    if plan.action == BuildAction::NotRequired {
        bail!("internal error: attempted to prepare a build marked not required");
    }
    let incremental_build_invoked = plan.action == BuildAction::IncrementalBuild;
    if let Some(preset) = plan.preset.as_deref().filter(|_| incremental_build_invoked) {
        let packed_environment = std::env::var_os("SIRIUS_RUNNER_PACKED").is_some();
        if packed_environment {
            prepare_packed_toolchain(repo_root, preset, reporter)?;
        }
        let mut command = if packed_environment {
            let mut command = Command::new("make");
            command.arg(preset);
            command
        } else {
            let mut command = Command::new("pixi");
            command.arg("run").arg("make").arg(preset);
            command
        };
        command.current_dir(repo_root).stdin(Stdio::null());
        process::run(
            &mut command,
            format!(
                "Running incremental `{preset}` build{}",
                if packed_environment {
                    " in the packed environment"
                } else {
                    ""
                }
            ),
            reporter,
        )?;
    } else {
        reporter.status(&format!(
            "Using existing build {}",
            plan.build_dir.display()
        ))?;
    }

    verify_file(&plan.duckdb_binary, true, "DuckDB executable")?;
    verify_file(&plan.extension, false, "Sirius extension")?;
    reporter.status("Hashing the exact benchmark artifacts")?;
    let identity = ArtifactIdentity {
        duckdb_binary_sha256: hashing::file_with_progress(
            &plan.duckdb_binary,
            "DuckDB executable",
            reporter,
        )?,
        extension_sha256: hashing::file_with_progress(
            &plan.extension,
            "Sirius extension",
            reporter,
        )?,
        config_sha256: config
            .map(|path| hashing::file_with_progress(path, "Sirius config", reporter))
            .transpose()?,
    };
    reporter.detail(&format!(
        "DuckDB: {} ({})",
        plan.duckdb_binary.display(),
        short_id(&identity.duckdb_binary_sha256)
    ))?;
    reporter.detail(&format!(
        "Sirius extension: {} ({})",
        plan.extension.display(),
        short_id(&identity.extension_sha256)
    ))?;

    let (source, git_commit, git_dirty) = if incremental_build_invoked {
        (
            "repository_incremental_build".to_string(),
            git_capture(repo_root, &["rev-parse", "HEAD"]),
            git_dirty(repo_root),
        )
    } else {
        ("external_build_directory".to_string(), None, None)
    };
    Ok(PreparedBuild {
        build_dir: plan.build_dir.clone(),
        duckdb_binary: plan.duckdb_binary.clone(),
        extension: plan.extension.clone(),
        identity,
        provenance: BuildProvenance {
            source,
            git_commit,
            git_dirty,
            preset: plan.preset.clone(),
            incremental_build_invoked,
            pixi_pack_key: std::env::var("SIRIUS_REMOTE_PACK_KEY").ok(),
        },
    })
}

fn prepare_packed_toolchain(
    repo_root: &Path,
    preset: &str,
    reporter: &mut impl Reporter,
) -> anyhow::Result<()> {
    reporter.status("Preparing the packed build toolchain")?;
    let prefix = PathBuf::from(
        std::env::var_os("CONDA_PREFIX")
            .context("the packed environment did not set CONDA_PREFIX")?,
    );
    prepare_clang_driver(&prefix)?;
    refresh_cmake_presets(repo_root)?;

    let mut configure = Command::new("cmake");
    configure
        .arg("--fresh")
        .arg("--preset")
        .arg(preset)
        .current_dir(repo_root.join("duckdb"))
        .stdin(Stdio::null());
    process::run(
        &mut configure,
        format!("Refreshing packed `{preset}` CMake configuration"),
        reporter,
    )
    .map(|_| ())
}

#[cfg(unix)]
fn refresh_cmake_presets(repo_root: &Path) -> anyhow::Result<()> {
    let source = repo_root.join("cmake/CMakePresets.json");
    let duckdb = repo_root.join("duckdb");
    ensure!(
        source.is_file() && duckdb.is_dir(),
        "the remote checkout is missing CMake presets or the DuckDB submodule"
    );
    let legacy_presets = duckdb.join("CMakePresets.json");
    if fs::symlink_metadata(&legacy_presets).is_ok() {
        fs::remove_file(&legacy_presets)
            .with_context(|| format!("removing stale {}", legacy_presets.display()))?;
    }
    let user_presets = duckdb.join("CMakeUserPresets.json");
    fs::write(
        &user_presets,
        "{\n  \"version\": 6,\n  \"include\": [\"../cmake/CMakePresets.json\"]\n}\n",
    )
    .with_context(|| format!("writing {}", user_presets.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn refresh_cmake_presets(_repo_root: &Path) -> anyhow::Result<()> {
    bail!("packed Sirius builds require a Unix host")
}

#[cfg(unix)]
fn prepare_clang_driver(prefix: &Path) -> anyhow::Result<()> {
    use std::os::unix::{fs::PermissionsExt, fs::symlink};

    let clang_cpp = prefix.join("bin/clang-cpp");
    let clang_pp = prefix.join("bin/clang++");
    ensure!(
        clang_cpp.is_file()
            && clang_cpp
                .metadata()
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0),
        "packed environment is missing executable {}",
        clang_cpp.display()
    );
    if !clang_pp.exists() {
        ensure!(
            fs::symlink_metadata(&clang_pp).is_err(),
            "packed environment contains a broken clang++ link at {}",
            clang_pp.display()
        );
        if let Err(error) = symlink("clang-cpp", &clang_pp)
            && error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error)
                .with_context(|| format!("creating packed compiler link {}", clang_pp.display()));
        }
    }
    ensure!(
        clang_pp.is_file()
            && clang_pp
                .metadata()
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0),
        "packed environment has no executable clang++ at {}",
        clang_pp.display()
    );
    Ok(())
}

#[cfg(not(unix))]
fn prepare_clang_driver(_prefix: &Path) -> anyhow::Result<()> {
    bail!("packed Sirius builds require a Unix host")
}

fn verify_file(path: &Path, executable: bool, label: &str) -> anyhow::Result<()> {
    if !path.is_file() {
        bail!("{label} is missing at {}", path.display());
    }
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        if path.metadata()?.permissions().mode() & 0o111 == 0 {
            bail!("{label} is not executable: {}", path.display());
        }
    }
    Ok(())
}

fn git_capture(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
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
    crate::repository::is_dirty(repo_root).ok()
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn packed_toolchain_adds_the_required_clang_driver_link() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let clang_cpp = bin.join("clang-cpp");
        fs::write(&clang_cpp, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&clang_cpp, fs::Permissions::from_mode(0o755)).unwrap();

        prepare_clang_driver(temp.path()).unwrap();
        prepare_clang_driver(temp.path()).unwrap();

        assert_eq!(
            fs::read_link(bin.join("clang++")).unwrap(),
            PathBuf::from("clang-cpp")
        );
        assert!(bin.join("clang++").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn packed_build_refreshes_the_repository_cmake_presets() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("cmake")).unwrap();
        fs::create_dir_all(temp.path().join("duckdb")).unwrap();
        fs::write(temp.path().join("cmake/CMakePresets.json"), "{}").unwrap();
        fs::write(temp.path().join("duckdb/CMakePresets.json"), "stale").unwrap();
        fs::write(temp.path().join("duckdb/CMakeUserPresets.json"), "stale").unwrap();

        refresh_cmake_presets(temp.path()).unwrap();

        assert!(!temp.path().join("duckdb/CMakePresets.json").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("duckdb/CMakeUserPresets.json")).unwrap(),
            "{\n  \"version\": 6,\n  \"include\": [\"../cmake/CMakePresets.json\"]\n}\n"
        );
    }

    #[test]
    fn missing_external_artifacts_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let plan = BuildPlan {
            preset: None,
            build_dir: temp.path().to_path_buf(),
            duckdb_binary: temp.path().join("duckdb"),
            extension: temp.path().join("sirius.duckdb_extension"),
            action: BuildAction::UseExisting,
        };
        let mut progress = crate::progress::Progress::with_writer(Vec::new(), 0);
        assert!(prepare(&plan, temp.path(), None, &mut progress).is_err());
    }
}
