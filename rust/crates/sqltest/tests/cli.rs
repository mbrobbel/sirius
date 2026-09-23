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
fn selection_replays_prior_queries_and_stops_after_selected_case() {
    let dir = corpus();
    fs::write(
        dir.path().join("suites/regressions/sequence.slt"),
        r#"statement ok
CREATE SEQUENCE counter START 1;

# sirius: id = "sequence/first"
# sirius: snapshot = true
query I
SELECT nextval('counter');
----
1

# sirius: id = "sequence/second"
# sirius: snapshot = true
query I
SELECT nextval('counter');
----
2

statement ok
SELECT * FROM must_not_execute;

# sirius: id = "sequence/later"
query I
SELECT 3;
----
"#,
    )
    .unwrap();
    let (passed, report) = run(
        dir.path(),
        "selected-sequence",
        &["--select", "sequence/second"],
    );
    assert!(passed, "{report}");
    assert_eq!(report["cases"].as_array().unwrap().len(), 1);
    let case = &report["cases"][0];
    let artifact = dir
        .path()
        .join("selected-sequence")
        .join(case["artifacts"].as_str().unwrap());
    let repro = fs::read_to_string(artifact.join("repro.sql")).unwrap();
    assert_eq!(repro.matches("nextval('counter')").count(), 2);
    assert!(!repro.contains("must_not_execute"));
    let file = dir.path().join("suites/regressions/sequence.slt");
    fs::write(
        &file,
        fs::read_to_string(&file)
            .unwrap()
            .replacen("----\n1", "----\n999", 1),
    )
    .unwrap();
    let (passed, report) = run(
        dir.path(),
        "failed-prerequisite",
        &["--select", "sequence/second"],
    );
    assert!(!passed);
    assert_eq!(report["cases"][0]["outcome"], "blocked");
    assert!(
        report["cases"][0]["message"]
            .as_str()
            .unwrap()
            .contains("sequence/first")
    );
}

#[test]
fn cpu_validation_and_completion_respect_setup_engine_conditions() {
    let dir = corpus();
    let file = dir.path().join("suites/regressions/conditions.slt");
    fs::write(
        &file,
        r#"onlyif sirius
statement ok
SET setting_only_in_sirius = true;

skipif sirius
statement ok
CREATE TABLE t AS SELECT 42 AS n;

# sirius: id = "conditions/value"
query I
SELECT n FROM t;
----
"#,
    )
    .unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .arg("complete")
            .arg(&file)
            .arg("--root")
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let (passed, report) = run(dir.path(), "conditions", &[]);
    assert!(passed, "{report}");
    let artifact = dir
        .path()
        .join("conditions")
        .join(report["cases"][0]["artifacts"].as_str().unwrap());
    for filename in ["repro.sql", "reference-repro.sql"] {
        let sql = fs::read_to_string(artifact.join(filename)).unwrap();
        assert!(!sql.contains("setting_only_in_sirius"));
        assert!(sql.contains("CREATE TABLE t"));
    }
}

#[test]
fn named_connections_preserve_snapshots_and_replay() {
    let dir = corpus();
    fs::write(
        dir.path().join("suites/regressions/suite.toml"),
        "storage = [\"cached\"]\ncheckpoint = \"explicit\"\n",
    )
    .unwrap();
    let file = dir.path().join("suites/regressions/mvcc.slt");
    fs::write(
        &file,
        r#"statement ok
CREATE TABLE t(i INTEGER);

statement ok
INSERT INTO t VALUES (1), (2);

statement ok
CHECKPOINT;

connection reader
statement ok
BEGIN TRANSACTION;

# sirius: id = "mvcc/before"
# sirius: snapshot = true
connection reader
query I
SELECT sum(i) FROM t;
----
3

statement ok
INSERT INTO t VALUES (3);

# sirius: id = "mvcc/old_snapshot"
# sirius: snapshot = true
connection reader
query I
SELECT sum(i) FROM t;
----
3

# sirius: id = "mvcc/current"
# sirius: snapshot = true
query I
SELECT sum(i) FROM t;
----
6

connection reader
statement ok
ROLLBACK;

connection reader
statement ok
BEGIN TRANSACTION;

connection reader
statement error Conversion Error
INSERT INTO t VALUES ('bad');

connection reader
statement ok
ROLLBACK;

# sirius: id = "mvcc/after"
# sirius: snapshot = true
connection reader
query I
SELECT sum(i) FROM t;
----
6
"#,
    )
    .unwrap();
    let (passed, report) = run(dir.path(), "mvcc", &[]);
    assert!(passed, "{report}");
    assert_eq!(report["cases"][1]["connection"]["named"], "reader");
    let artifacts = dir
        .path()
        .join("mvcc")
        .join(report["cases"][3]["artifacts"].as_str().unwrap());
    assert!(!artifacts.join("repro.sql").exists());
    assert!(artifacts.join("repro.slt").is_file());
    let binding = format!("replay={}", artifacts.display());
    let (passed, replay) = run(
        dir.path(),
        "replay",
        &[
            "--suite-dir",
            &binding,
            "--suite",
            "replay",
            "--select",
            "mvcc/after",
        ],
    );
    assert!(passed, "{replay}");
    assert_eq!(replay["cases"].as_array().unwrap().len(), 1);
    for file in [file, artifacts.join("repro.slt")] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
                .arg("complete")
                .arg(&file)
                .arg("--root")
                .arg(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let completed = fs::read_to_string(file).unwrap();
        assert!(completed.contains("connection reader"));
        assert_eq!(completed.matches("\n3\n").count(), 2);
        assert_eq!(completed.matches("\n6\n").count(), 2);
    }
}

#[test]
fn sweep_settings_apply_to_every_named_connection() {
    let dir = corpus();
    let manifest = dir.path().join("sqltest.toml");
    fs::write(&manifest, fs::read_to_string(&manifest).unwrap().replace("settings = { disabled_optimizers = \"\" }", "settings = { disabled_optimizers = \"join_order\" }\nreference_settings = { disabled_optimizers = \"join_order\" }")).unwrap();
    fs::write(dir.path().join("suites/regressions/settings.slt"), "# sirius: id = \"settings/named\"\n# sirius: snapshot = true\nconnection other\nquery T\nSELECT current_setting('disabled_optimizers');\n----\n\"join_order\"\n").unwrap();
    let (passed, report) = run(dir.path(), "named-settings", &[]);
    assert!(passed, "{report}");
}

#[test]
fn generated_files_are_isolated_between_workers() {
    let dir = corpus();
    fs::write(dir.path().join("suites/regressions/files.slt"), "statement ok\nCOPY (SELECT 17 AS value) TO '__TEST_DIR__/nested/data.parquet' (FORMAT PARQUET);\n\n# sirius: id = \"files/read\"\n# sirius: snapshot = true\nquery I\nSELECT value FROM read_parquet('__TEST_DIR__/nested/data.parquet');\n----\n17\n").unwrap();
    fs::write(
        dir.path().join("suites/regressions/suite.toml"),
        "scratch_directories = [\"nested\"]\n",
    )
    .unwrap();
    let (passed, report) = run(dir.path(), "files", &[]);
    assert!(passed, "{report}");
    let artifacts = dir
        .path()
        .join("files")
        .join(report["cases"][0]["artifacts"].as_str().unwrap());
    let saved = fs::read_to_string(artifacts.join("suite.toml")).unwrap();
    assert!(saved.contains("nested"));
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .arg("complete")
            .arg(artifacts.join("repro.slt"))
            .arg("--root")
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    let reference = fs::read_to_string(artifacts.join("reference-repro.sql")).unwrap();
    let actual = fs::read_to_string(artifacts.join("repro.sql")).unwrap();
    assert!(reference.contains("/reference/nested/data.parquet"));
    assert!(!reference.contains("/sirius/nested/data.parquet"));
    assert!(actual.contains("/sirius/nested/data.parquet"));
    assert!(!actual.contains("/reference/nested/data.parquet"));
    let work = fs::read_to_string(artifacts.join("worker-logs.txt")).unwrap();
    let work = dir.path().join("files").join(work.trim());
    assert!(work.join("reference/nested/data.parquet").is_file());
    assert!(work.join("sirius/nested/data.parquet").is_file());
}

#[test]
fn setup_failure_after_final_query_cannot_pass() {
    let dir = corpus();
    fs::write(dir.path().join("suites/regressions/cleanup.slt"), "# sirius: id = \"cleanup/query\"\nquery I\nSELECT 1;\n----\n\nstatement ok\nSELECT * FROM missing_cleanup_table;\n").unwrap();
    let (passed, report) = run(dir.path(), "cleanup", &[]);
    assert!(!passed);
    assert_eq!(report["complete"], false);
    assert_eq!(report["cases"][0]["outcome"], "infrastructure_failure");
    let artifact = dir
        .path()
        .join("cleanup")
        .join(report["cases"][0]["artifacts"].as_str().unwrap());
    assert!(
        fs::read_to_string(artifact.join("repro.sql"))
            .unwrap()
            .contains("missing_cleanup_table")
    );
}

#[test]
fn fallback_policy_survives_completion_and_is_reported_separately() {
    let dir = corpus();
    let file = dir.path().join("suites/regressions/policy.slt");
    fs::write(&file, "# sirius: id = \"policy/allowed\"\n# sirius: execution = \"allow_fallback\"\nquery I\nSELECT 1;\n----\n\n# sirius: id = \"policy/default\"\nquery I\nSELECT 2;\n----\n").unwrap();
    assert!(
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .arg("complete")
            .arg(&file)
            .arg("--root")
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("execution = \"allow_fallback\"")
    );
    let (passed, report) = run(dir.path(), "policy", &[]);
    assert!(passed, "{report}");
    assert_eq!(report["cases"][0]["execution"], "allow_fallback");
    assert_eq!(report["cases"][1]["execution"], "no_fallback");
    let summary = fs::read_to_string(dir.path().join("policy/summary.md")).unwrap();
    assert!(summary.contains("fallback allowed | 1 | 1"));
    assert!(summary.contains("fallback disabled | 1 | 1"));
    fs::write(
        &file,
        fs::read_to_string(&file)
            .unwrap()
            .replace("allow_fallback", "allow_typo"),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args(["run", "--dry-run", "--root"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("unknown variant"));
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
