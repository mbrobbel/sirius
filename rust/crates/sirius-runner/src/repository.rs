use std::{fs, path::Path, process::Command};

use anyhow::{Context, ensure};

const RELEVANT_SUBMODULES: &[&str] = &["duckdb", "cucascade", "substrait"];

pub(crate) fn git_output(repository: &Path, arguments: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .current_dir(repository)
        .args(arguments)
        .output()
        .with_context(|| format!("running Git in {}", repository.display()))?;
    ensure!(
        output.status.success(),
        "Git command failed in {}: {}",
        repository.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)
        .context("Git returned non-UTF-8 output")?
        .trim()
        .to_owned())
}

pub(crate) fn is_dirty(repository: &Path) -> anyhow::Result<bool> {
    let root_status = git_output(
        repository,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=dirty",
        ],
    )?;
    if !root_status.is_empty() {
        return Ok(true);
    }

    for name in RELEVANT_SUBMODULES {
        let submodule = repository.join(name);
        if fs::symlink_metadata(submodule.join(".git")).is_err() {
            continue;
        }
        let status = git_output(
            &submodule,
            &["status", "--porcelain=v1", "--untracked-files=normal"],
        )?;
        for line in status.lines().filter(|line| !line.is_empty()) {
            if *name == "duckdb"
                && line == "?? CMakePresets.json"
                && is_legacy_preset_link(repository, &submodule.join("CMakePresets.json"))
            {
                continue;
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_legacy_preset_link(repository: &Path, path: &Path) -> bool {
    let Ok(target) = fs::read_link(path) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else {
        path.parent().unwrap_or(repository).join(target)
    };
    let expected = repository.join("cmake/CMakePresets.json");
    fs::canonicalize(target).ok() == fs::canonicalize(expected).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn only_the_exact_legacy_preset_link_is_recognized() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("cmake")).unwrap();
        fs::create_dir(temp.path().join("duckdb")).unwrap();
        fs::write(temp.path().join("cmake/CMakePresets.json"), "{}").unwrap();
        let link = temp.path().join("duckdb/CMakePresets.json");
        symlink("../cmake/CMakePresets.json", &link).unwrap();
        assert!(is_legacy_preset_link(temp.path(), &link));

        fs::remove_file(&link).unwrap();
        fs::write(&link, "{}").unwrap();
        assert!(!is_legacy_preset_link(temp.path(), &link));
    }
}
