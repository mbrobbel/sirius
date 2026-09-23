use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn host_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::create_dir_all(root.join(".pixi/envs/default/lib")).unwrap();
    fs::write(root.join(".pixi/envs/default/lib/libcuvs.so"), b"").unwrap();
    fs::create_dir_all(root.join("duckdb/lib")).unwrap();
    fs::write(root.join("duckdb/lib/libduckdb.so"), b"").unwrap();
    fs::write(root.join("extension"), b"").unwrap();
    fs::copy(
        env!("CARGO_BIN_EXE_sirius-sqltest-host"),
        root.join("bin/sirius-sqltest-host"),
    )
    .unwrap();
    executable(&root.join("bin/ldd"), "#!/bin/sh\nexit 0\n");
    executable(
        &root.join("bin/sirius-sqltest"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> commands.log
printf '%s\n' "$LD_LIBRARY_PATH" >> libraries.log
if [ "$1" = run ] && [ "$3" = first ]; then exit 1; fi
if [ "$1" = prepare ] && [ "$3" = second ]; then exit 2; fi
exit 0
"#,
    );
    dir
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(root.join("bin/sirius-sqltest-host"));
    command
        .current_dir(root)
        .env("PATH", root.join("bin"))
        .env_remove("LD_LIBRARY_PATH")
        .env("DUCKDB_LIB_DIR", root.join("duckdb/lib"))
        .args(["--extension", "extension", "first", "second", "third"]);
    command
}

#[test]
fn host_bootstraps_without_duckdb_and_continues_after_failed_runs() {
    let dir = host_fixture();
    let output = command(dir.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let commands = fs::read_to_string(dir.path().join("commands.log")).unwrap();
    let commands: Vec<_> = commands.lines().collect();
    assert_eq!(commands.len(), 5);
    assert_eq!(commands[0], "prepare --run first");
    assert!(commands[1].starts_with("run --run first --extension "));
    assert!(!commands[1].contains("--runtime-revision"));
    assert_eq!(commands[2], "prepare --run second");
    assert_eq!(commands[3], "prepare --run third");
    assert!(commands[4].starts_with("run --run third --extension "));
    let libraries = fs::read_to_string(dir.path().join("libraries.log")).unwrap();
    for line in libraries.lines() {
        let paths: Vec<_> = std::env::split_paths(line).collect();
        assert_eq!(paths[0], dir.path().join("duckdb/lib"));
        assert_eq!(paths[1], dir.path().join(".pixi/envs/default/lib"));
    }
    assert_eq!(fs::read_dir(dir.path().join("runs")).unwrap().count(), 1);
}

#[test]
fn missing_shared_library_stops_before_any_preparation() {
    let dir = host_fixture();
    executable(
        &dir.path().join("bin/ldd"),
        "#!/bin/sh\nprintf 'libcuda.so.1 => not found\\n'\nexit 0\n",
    );
    let output = command(dir.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("libcuda.so.1 => not found"));
    assert!(!dir.path().join("commands.log").exists());
    assert!(!dir.path().join("runs").exists());
}

#[test]
fn explicit_duckdb_directory_overrides_the_environment() {
    let dir = host_fixture();
    let output = command(dir.path())
        .env("DUCKDB_LIB_DIR", dir.path().join("missing"))
        .args(["--duckdb-lib-dir", "duckdb/lib"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(dir.path().join("commands.log").is_file());

    let dir = host_fixture();
    let output = command(dir.path())
        .env_remove("DUCKDB_LIB_DIR")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--duckdb-lib-dir"));
    assert!(!dir.path().join("commands.log").exists());
}
