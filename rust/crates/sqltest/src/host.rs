mod name;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use name::Name;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Parser)]
#[command(about = "Run SQL suites on the host using a local or container Pixi environment")]
struct Args {
    #[arg(long)]
    extension: PathBuf,
    #[arg(long)]
    gpu_lib_dir: Option<PathBuf>,
    /// DuckDB library directory; defaults to the activated Pixi environment.
    #[arg(long)]
    duckdb_lib_dir: Option<PathBuf>,
    /// Named runs from sqltest.toml, executed sequentially.
    #[arg(required = true)]
    runs: Vec<Name>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerMount {
    source: Option<PathBuf>,
    destination: PathBuf,
}

fn checked_output(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("start {command:?}"))?;
    ensure!(
        output.status.success(),
        "{command:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).context("command output is not UTF-8")
}

fn mounted_library_directory(root: &Path, mounts: &str) -> Result<PathBuf> {
    let mut candidates = BTreeSet::new();
    for line in mounts.lines() {
        let mounts: Vec<DockerMount> =
            serde_json::from_str(line).context("decode Docker mounts")?;
        for mount in mounts {
            if mount.destination == root.join(".pixi")
                && let Some(source) = mount.source
            {
                let directory = source.join("envs/default/lib");
                if directory.join("libcuvs.so").is_file() {
                    candidates.insert(directory.canonicalize()?);
                }
            }
        }
    }
    ensure!(
        candidates.len() == 1,
        "Cannot identify one readable GPU library directory for this worktree. \
         Pass --gpu-lib-dir PATH (the directory containing libcuvs.so). \
         Matching directories: {candidates:?}"
    );
    Ok(candidates.pop_first().expect("one candidate"))
}

fn gpu_library_directory(root: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(directory) = explicit {
        ensure!(
            directory.join("libcuvs.so").is_file(),
            "GPU library directory has no libcuvs.so: {}",
            directory.display()
        );
        return Ok(directory.canonicalize()?);
    }
    let local = root.join(".pixi/envs/default/lib");
    if local.join("libcuvs.so").is_file() {
        return Ok(local);
    }
    let containers = checked_output(Command::new("docker").args(["ps", "-q"]))?;
    let containers: Vec<_> = containers.split_whitespace().collect();
    let mounts = if containers.is_empty() {
        String::new()
    } else {
        checked_output(
            Command::new("docker")
                .args(["inspect", "--format", "{{json .Mounts}}"])
                .args(containers),
        )?
    };
    mounted_library_directory(root, &mounts)
}

fn library_path(
    runtime: &Path,
    gpu: &Path,
    driver: &Path,
    inherited: Option<&OsStr>,
) -> Result<OsString> {
    let mut directories = vec![runtime.to_path_buf(), gpu.to_path_buf()];
    if driver.is_dir() {
        directories.push(driver.to_path_buf());
    }
    if let Some(inherited) = inherited.filter(|value| !value.is_empty()) {
        directories.extend(env::split_paths(inherited));
    }
    Ok(env::join_paths(directories)?)
}

fn check_dependencies(paths: &[&Path], libraries: &OsStr) -> Result<()> {
    for path in paths {
        ensure!(path.is_file(), "Missing built artifact: {}", path.display());
        let output = Command::new("ldd")
            .arg(path)
            .env("LD_LIBRARY_PATH", libraries)
            .output()
            .context("run shared-library preflight")?;
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(
            output.status.success() && !detail.contains("not found"),
            "Shared-library check failed for {}:\n{detail}",
            path.display()
        );
    }
    Ok(())
}

fn execute(args: Args) -> Result<bool> {
    ensure!(
        args.runs.iter().collect::<BTreeSet<_>>().len() == args.runs.len(),
        "run names must be distinct"
    );
    let root = env::current_dir()?.canonicalize()?;
    let runtime = args
        .duckdb_lib_dir
        .or_else(|| env::var_os("DUCKDB_LIB_DIR").map(PathBuf::from))
        .context("use pixi run or pass --duckdb-lib-dir PATH")?
        .canonicalize()
        .context("locate DuckDB library directory")?;
    ensure!(
        runtime.join("libduckdb.so").is_file(),
        "DuckDB library directory has no libduckdb.so: {}",
        runtime.display()
    );
    let gpu = gpu_library_directory(&root, args.gpu_lib_dir.as_deref())?;
    let libraries = library_path(
        &runtime,
        &gpu,
        Path::new("/run/opengl-driver/lib"),
        env::var_os("LD_LIBRARY_PATH").as_deref(),
    )?;
    let runner = env::current_exe()?.with_file_name("sirius-sqltest");
    let extension = args.extension.canonicalize().context("locate extension")?;
    check_dependencies(&[&runner, &extension], &libraries)?;
    fs::create_dir_all(root.join("runs"))?;
    let output = tempfile::Builder::new()
        .prefix("host-")
        .tempdir_in(root.join("runs"))?
        .keep();
    println!(
        "GPU libraries: {}\nResults: {}",
        gpu.display(),
        output.display()
    );
    let mut success = true;
    for name in args.runs {
        let prepared = Command::new(&runner)
            .args(["prepare", "--run", name.as_ref()])
            .env("LD_LIBRARY_PATH", &libraries)
            .status()?;
        if !prepared.success() {
            success = false;
            continue;
        }
        let result = Command::new(&runner)
            .args(["run", "--run", name.as_ref(), "--extension"])
            .arg(&extension)
            .arg("--output")
            .arg(output.join(name.as_ref()))
            .env("LD_LIBRARY_PATH", &libraries)
            .status()?;
        success &= result.success();
    }
    println!("Results: {}", output.display());
    Ok(success)
}

fn main() {
    match execute(Args::parse()) {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(path: &Path) {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("libcuvs.so"), b"").unwrap();
    }

    #[test]
    fn local_and_explicit_paths_do_not_require_docker() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(".pixi/envs/default/lib");
        let explicit = dir.path().join("explicit/lib");
        library(&local);
        library(&explicit);
        assert_eq!(gpu_library_directory(dir.path(), None).unwrap(), local);
        assert_eq!(
            gpu_library_directory(dir.path(), Some(&explicit)).unwrap(),
            explicit
        );
        assert!(gpu_library_directory(dir.path(), Some(&dir.path().join("missing"))).is_err());
    }

    #[test]
    fn mounted_libraries_are_scoped_to_the_worktree_and_must_be_unambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("worktree with spaces");
        let first = dir.path().join("first volume");
        let second = dir.path().join("second volume");
        for source in [&first, &second] {
            library(&source.join("envs/default/lib"));
        }
        let mounts = serde_json::json!([
            {"Source": second, "Destination": "/another/.pixi"},
            {"Source": first, "Destination": root.join(".pixi")},
            {"Destination": "/data", "Type": "tmpfs"}
        ]);
        assert_eq!(
            mounted_library_directory(&root, &format!("{mounts}\n[]")).unwrap(),
            first.join("envs/default/lib")
        );
        let conflicting = serde_json::json!([
            {"Source": second, "Destination": root.join(".pixi")}
        ]);
        assert!(mounted_library_directory(&root, &format!("{mounts}\n{conflicting}")).is_err());
        assert!(mounted_library_directory(&root, "").is_err());
    }

    #[test]
    fn driver_path_is_optional_and_inherited_search_paths_are_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("opengl-driver/lib");
        let runtime = dir.path().join("runtime");
        let gpu = dir.path().join("gpu");
        let inherited = OsStr::new("/user/lib:/other/lib");
        assert_eq!(
            env::split_paths(&library_path(&runtime, &gpu, &driver, None).unwrap())
                .collect::<Vec<_>>(),
            [runtime.clone(), gpu.clone()]
        );
        fs::create_dir_all(&driver).unwrap();
        assert_eq!(
            env::split_paths(&library_path(&runtime, &gpu, &driver, Some(inherited)).unwrap())
                .collect::<Vec<_>>(),
            [
                runtime,
                gpu,
                driver,
                PathBuf::from("/user/lib"),
                PathBuf::from("/other/lib")
            ]
        );
    }
}
