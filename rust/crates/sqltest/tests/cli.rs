use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn corpus() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("suites/regressions")).unwrap();
    fs::write(
        dir.path().join("sqltest.toml"),
        r#"version = 1
default_run = "quick"
suite_roots = ["suites"]
[storage.cached]
relation = "table"
[storage.external]
relation = "view"
[profiles.desktop]
gpus = 1
config = "config.yaml"
[axes.machine.desktop]
profile = "desktop"
[axes.backend.cached]
storage = "cached"
[axes.backend.external]
storage = "external"
[axes.optimizers.default]
settings = { disabled_optimizers = "" }
[axes.optimizers.fixed]
settings = { disabled_optimizers = "join_order" }
[runs.quick]
suites = "all"
axes = { machine = ["desktop"], backend = ["cached"], optimizers = ["default"] }
"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("suites/regressions/suite.toml"),
        "storage = [\"cached\"]\n",
    )
    .unwrap();
    fs::write(dir.path().join("config.yaml"), "{}\n").unwrap();
    fs::write(dir.path().join("gaps.toml"), "gaps = []\n").unwrap();
    dir
}

fn run(root: &Path, output: &str, extra: &[&str]) -> (bool, Value) {
    let output = root.join(output);
    let status = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args(["run", "--cpu-only", "--root"])
        .arg(root)
        .arg("--output")
        .arg(&output)
        .args(extra)
        .status()
        .unwrap();
    let report = serde_json::from_slice(&fs::read(output.join("report.json")).unwrap()).unwrap();
    (status.success(), report)
}

#[test]
fn completion_includes_and_selection_roundtrip() {
    let dir = corpus();
    let root = dir.path();
    let fixture = "statement ok\nCREATE TABLE t(i BIGINT, s VARCHAR);\n\nstatement count 4\nINSERT INTO t VALUES (9007199254740993, 'NULL'), (NULL, NULL), (1, ''), (1, 'a' || chr(9) || 'b');\n";
    fs::write(root.join("fixture.slt"), fixture).unwrap();
    let file = root.join("suites/regressions/example.slt");
    fs::write(&file, "include ../../fixture.slt\n\n# sirius: id = \"regressions/values\"\n# sirius: snapshot = false\nquery II rowsort\nSELECT * FROM t;\n----\n\n# sirius: id = \"regressions/error\"\nquery error does not exist\nSELECT * FROM missing_table;\n").unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .arg("complete")
            .arg(&file)
            .arg("--root")
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(root.join("fixture.slt")).unwrap(),
        fixture
    );
    let completed = fs::read_to_string(&file).unwrap();
    assert!(completed.contains("snapshot = true"));
    assert!(completed.contains("9007199254740993\t\"NULL\""));
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .arg("complete")
            .arg(&file)
            .arg("--root")
            .arg(root)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), completed);
    let (passed, report) = run(root, "completed", &["--axis", "backend=all"]);
    assert!(passed, "{report}");
    assert_eq!(report["cases"].as_array().unwrap().len(), 2);
    assert_eq!(report["provenance"]["cpu_only"], true);
    let (passed, report) = run(root, "selected", &["--select", "regressions/values"]);
    assert!(passed, "{report}");
    assert_eq!(report["cases"].as_array().unwrap().len(), 1);
}

#[test]
fn reference_timeout_blocks_file_but_continues_next_file() {
    let dir = corpus();
    fs::write(dir.path().join("suites/regressions/a.slt"), "# sirius: id = \"regressions/timeout\"\n# sirius: timeout = 1\nquery I\nSELECT sum(sin(i)) FROM range(1000000000000) t(i);\n----\n\n# sirius: id = \"regressions/blocked\"\nquery I\nSELECT 1;\n----\n").unwrap();
    fs::write(
        dir.path().join("suites/regressions/b.slt"),
        "# sirius: id = \"regressions/independent\"\nquery I\nSELECT 1;\n----\n",
    )
    .unwrap();
    let (passed, report) = run(dir.path(), "timeout", &[]);
    assert!(!passed);
    assert_eq!(report["complete"], false);
    let outcomes: Vec<_> = report["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["reference_failure", "blocked", "match"]);
}

#[test]
fn ambiguous_floats_are_a_harness_limitation() {
    let dir = corpus();
    fs::write(dir.path().join("suites/regressions/float.slt"), "# sirius: id = \"regressions/float\"\n# sirius: tolerances = { \"1\" = { absolute = 1e-8, relative = 1e-9 } }\nquery IR rowsort\nSELECT 1, i::DOUBLE FROM range(2) t(i);\n----\n").unwrap();
    let (passed, report) = run(dir.path(), "ambiguous", &[]);
    assert!(!passed);
    assert_eq!(report["cases"][0]["outcome"], "harness_error");
    assert_eq!(report["cases"][0]["reference_rows"], 2);
}

#[test]
fn manifest_recipes_drive_new_suites_and_storage_modes() {
    let dir = corpus();
    let root = dir.path();
    let path = root.join("sqltest.toml");
    let manifest = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{manifest}\n[fixtures.tiny]\nsql = [\"tiny.sql\"]\ntables = [\"numbers\"]\n"),
    )
    .unwrap();
    fs::write(
        root.join("suites/regressions/suite.toml"),
        "storage = [\"cached\", \"external\"]\nfixtures = [\"tiny\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("tiny.sql"),
        "CREATE TABLE numbers AS SELECT * FROM range(3);\n",
    )
    .unwrap();
    fs::write(root.join("suites/regressions/recipe.slt"), "statement ok\nCREATE __RELATION__ numbers AS SELECT * FROM read_parquet('__FIXTURE_ROOT__/tiny/numbers.parquet');\n\n# sirius: id = \"custom/recipe\"\nquery I\nSELECT count(*) FROM numbers;\n----\n").unwrap();
    let fixtures = root.join("data");
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(["prepare", "--root"])
            .arg(root)
            .arg("--output")
            .arg(&fixtures)
            .status()
            .unwrap()
            .success()
    );
    let (passed, report) = run(
        root,
        "recipes",
        &[
            "--fixtures",
            fixtures.to_str().unwrap(),
            "--axis",
            "backend=all",
        ],
    );
    assert!(passed, "{report}");
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    assert_eq!(cases[0]["profile"], "desktop");
    assert_eq!(cases[0]["storage"], "cached");
    assert_eq!(cases[1]["storage"], "external");
    fs::write(
        root.join("tiny.sql"),
        "CREATE TABLE numbers AS SELECT * FROM range(4);\n",
    )
    .unwrap();
    let (passed, report) = run(
        root,
        "stale-recipe",
        &["--fixtures", fixtures.to_str().unwrap()],
    );
    assert!(!passed);
    assert_eq!(report["cases"][0]["outcome"], "infrastructure_failure");
    assert!(
        report["cases"][0]["message"]
            .as_str()
            .unwrap()
            .contains("recipe changed")
    );
}

#[test]
fn dry_run_checks_references_before_starting_workers() {
    let dir = corpus();
    fs::write(
        dir.path().join("suites/regressions/query.slt"),
        "# sirius: id = \"custom/query\"\nquery I\nSELECT 1;\n----\n",
    )
    .unwrap();
    let dry_run = || {
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(["run", "--dry-run", "--root"])
            .arg(dir.path())
            .output()
            .unwrap()
    };
    let good = dry_run();
    assert!(good.status.success());
    let plan: Value = serde_json::from_slice(&good.stdout).unwrap();
    assert_eq!(plan["cases"], 1);
    let path = dir.path().join("sqltest.toml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("profile = \"desktop\"", "profile = \"typo\""),
    )
    .unwrap();
    let bad = dry_run();
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown profile typo"));
}

#[test]
fn sweeps_apply_settings_and_keep_results_distinct() {
    let dir = corpus();
    let root = dir.path();
    let path = root.join("sqltest.toml");
    fs::write(&path, format!("{}\n[axes.workers.single]\nsettings = {{ threads = 1 }}\n[axes.workers.pair]\nsettings = {{ threads = 2 }}\n", fs::read_to_string(&path).unwrap())).unwrap();
    fs::write(root.join("suites/regressions/settings.slt"), "# sirius: id = \"settings/optimizer\"\nquery T\nSELECT current_setting('disabled_optimizers');\n----\n").unwrap();
    let (passed, report) = run(
        root,
        "sweep",
        &["--axis", "optimizers=all", "--axis", "workers=all"],
    );
    assert!(!passed);
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    assert_eq!(cases.iter().filter(|c| c["outcome"] == "match").count(), 2);
    assert_eq!(
        cases.iter().filter(|c| c["outcome"] == "mismatch").count(),
        2
    );
    let paths: std::collections::BTreeSet<_> = cases
        .iter()
        .map(|c| c["artifacts"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 4);
    for case in cases {
        let settings = root
            .join("sweep")
            .join(case["artifacts"].as_str().unwrap())
            .join("sirius-settings.sql");
        assert!(fs::read_to_string(settings).unwrap().contains("threads"));
    }
}

#[test]
fn generated_test_directories_need_no_registration() {
    let dir = corpus();
    let generated = tempfile::tempdir().unwrap();
    fs::write(generated.path().join("seed-17.slt"), "statement ok\nCREATE TABLE t(i INTEGER);\n\nstatement ok\nINSERT INTO t VALUES (-1), (NULL), (4);\n\n# sirius: id = \"generated/seed-17\"\n# sirius: snapshot = true\nquery I\nSELECT sum(i) FROM t;\n----\n3\n").unwrap();
    let binding = format!("generated={}", generated.path().display());
    let (passed, report) = run(
        dir.path(),
        "generated",
        &["--suite-dir", &binding, "--suite", "generated"],
    );
    assert!(passed, "{report}");
    assert_eq!(report["cases"].as_array().unwrap().len(), 1);
    assert_eq!(report["cases"][0]["suite"], "generated");
}

#[test]
fn conflicting_sweep_settings_fail_before_execution() {
    let dir = corpus();
    fs::write(
        dir.path().join("suites/regressions/query.slt"),
        "# sirius: id = \"custom/query\"\nquery I\nSELECT 1;\n----\n",
    )
    .unwrap();
    let path = dir.path().join("sqltest.toml");
    fs::write(&path, format!("{}\n[axes.first.value]\nsettings = {{ threads = 1 }}\n[axes.second.value]\nsettings = {{ threads = 2 }}\n", fs::read_to_string(&path).unwrap())).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args([
            "run",
            "--dry-run",
            "--axis",
            "first=all",
            "--axis",
            "second=all",
            "--root",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("conflicting values for setting threads")
    );
}
