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
fn suite_gpu_requirement_filters_incompatible_profiles() {
    let dir = corpus();
    let root = dir.path();
    fs::write(
        root.join("suites/regressions/suite.toml"),
        "minimum_gpus = 2\n",
    )
    .unwrap();
    fs::write(
        root.join("suites/regressions/query.slt"),
        "# sirius: id = \"multi/query\"\nquery I\nSELECT 1;\n----\n1\n",
    )
    .unwrap();
    let manifest = root.join("sqltest.toml");
    fs::write(&manifest, format!("{}\n[profiles.pair]\ngpus = 2\nconfig = \"config.yaml\"\n[axes.machine.pair]\nprofile = \"pair\"\n", fs::read_to_string(&manifest).unwrap())).unwrap();
    let dry_run = |choices: &str| {
        Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(["run", "--dry-run", "--axis", choices, "--root"])
            .arg(root)
            .output()
            .unwrap()
    };
    let single = dry_run("machine=desktop");
    assert_eq!(single.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&single.stderr).contains("no compatible suite/storage/GPU"));
    let sweep = dry_run("machine=all");
    assert!(sweep.status.success());
    let plan: Value = serde_json::from_slice(&sweep.stdout).unwrap();
    assert_eq!(plan["cases"], 1);
    assert_eq!(plan["plan"]["targets"][0]["profile"], "pair");
}

#[test]
fn profile_environment_applies_to_candidate_and_is_recorded() {
    let dir = corpus();
    let root = dir.path();
    let manifest = root.join("sqltest.toml");
    let original = fs::read_to_string(&manifest).unwrap();
    fs::write(
        root.join("suites/regressions/query.slt"),
        "# sirius: id = \"environment/query\"\nquery I\nSELECT 1;\n----\n1\n",
    )
    .unwrap();
    let (passed, baseline) = run(root, "baseline", &[]);
    assert!(passed);
    let cache = root.join("candidate-extensions");
    let environment = format!(
        "environment = {{ SIRIUS_SQLTEST_EXTENSION_DIR = {:?} }}\n",
        cache.to_str().unwrap()
    );
    fs::write(
        &manifest,
        original.replace(
            "[profiles.desktop]\n",
            &format!("[profiles.desktop]\n{environment}"),
        ),
    )
    .unwrap();
    let (passed, report) = run(root, "environment", &[]);
    assert!(passed, "{report}");
    assert!(cache.is_dir());
    let case = &report["cases"][0];
    assert_ne!(case["fingerprint"], baseline["cases"][0]["fingerprint"]);
    let artifact = root
        .join("environment")
        .join(case["artifacts"].as_str().unwrap());
    let recorded: Value =
        serde_json::from_slice(&fs::read(artifact.join("environment.json")).unwrap()).unwrap();
    assert_eq!(
        recorded["SIRIUS_SQLTEST_EXTENSION_DIR"],
        cache.to_str().unwrap()
    );
    for key in ["SIRIUS_DISABLE", "SIRIUS_CONFIG_FILE", "bad-name"] {
        fs::write(
            &manifest,
            original.replace(
                "[profiles.desktop]\n",
                &format!("[profiles.desktop]\nenvironment = {{ {key} = \"1\" }}\n"),
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(["run", "--dry-run", "--root"])
            .arg(root)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("environment variable"));
    }
}

#[test]
fn excluded_suites_remain_discoverable_and_explicitly_selectable() {
    let dir = corpus();
    let root = dir.path();
    for suite in ["regressions", "specialized"] {
        let path = root.join("suites").join(suite);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("suite.toml"), "storage = [\"cached\"]\n").unwrap();
        fs::write(
            path.join("query.slt"),
            format!("# sirius: id = \"{suite}/query\"\nquery I\nSELECT 1;\n----\n1\n"),
        )
        .unwrap();
    }
    let manifest = root.join("sqltest.toml");
    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap().replace(
            "suites = \"all\"",
            "suites = \"all\"\nexclude_suites = [\"specialized\"]",
        ),
    )
    .unwrap();
    for (selection, expected) in [
        (vec![], 1),
        (vec!["--suite", "specialized"], 1),
        (vec!["--suite", "all"], 2),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(["run", "--dry-run", "--root"])
            .arg(root)
            .args(selection)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(plan["cases"], expected);
    }
    fs::write(
        &manifest,
        fs::read_to_string(&manifest).unwrap().replace(
            "exclude_suites = [\"specialized\"]",
            "exclude_suites = [\"typo\"]",
        ),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args(["run", "--dry-run", "--root"])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown excluded suite typo"));
}

#[test]
fn correctness_runs_use_fixed_suite_profiles_without_sweeps() {
    let dir = corpus();
    let root = dir.path();
    let manifest = root.join("sqltest.toml");
    fs::write(&manifest, format!("{}\n[profiles.small]\ngpus = 1\nconfig = \"config.yaml\"\n[runs.correctness]\nsuites = \"all\"\nprofile = \"desktop\"\n", fs::read_to_string(&manifest).unwrap())).unwrap();
    fs::write(
        root.join("suites/regressions/suite.toml"),
        "storage = [\"cached\", \"external\"]\n",
    )
    .unwrap();
    fs::write(
        root.join("suites/regressions/query.slt"),
        "# sirius: id = \"regular/query\"\nquery I\nSELECT 1;\n----\n",
    )
    .unwrap();
    fs::create_dir(root.join("suites/small")).unwrap();
    fs::write(
        root.join("suites/small/suite.toml"),
        "storage = [\"cached\"]\nprofile = \"small\"\n",
    )
    .unwrap();
    fs::write(
        root.join("suites/small/query.slt"),
        "# sirius: id = \"small/query\"\nquery I\nSELECT 2;\n----\n",
    )
    .unwrap();
    let (passed, report) = run(root, "fixed", &["--run", "correctness"]);
    assert!(passed, "{report}");
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    let artifacts: std::collections::BTreeSet<_> = cases
        .iter()
        .map(|c| c["artifacts"].as_str().unwrap())
        .collect();
    assert_eq!(artifacts.len(), 3);
    for case in cases {
        assert!(case["axes"].as_object().unwrap().is_empty());
        assert_eq!(
            case["profile"],
            if case["suite"] == "small" {
                "small"
            } else {
                "desktop"
            }
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args([
            "run",
            "--dry-run",
            "--run",
            "correctness",
            "--axis",
            "machine=all",
            "--root",
        ])
        .arg(root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("select a sweep run"));
    let (passed, report) = run(root, "sweep", &["--run", "quick"]);
    assert!(passed, "{report}");
    assert!(
        report["cases"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["profile"] == "desktop")
    );
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
