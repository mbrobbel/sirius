use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn extension_errors_cannot_satisfy_expected_sql_errors() {
    let directory = tempfile::tempdir().unwrap();
    let extension = directory.path().join("invalid'artifact.duckdb_extension");
    let test = directory.path().join("test.slt");
    fs::write(&extension, "not an extension").unwrap();
    fs::write(&test, "statement error\nSELECT * FROM missing_table\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .arg("--extension")
        .arg(extension)
        .arg(test)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        output_text(&output).contains("0 passed; 1 failed"),
        "{}",
        output_text(&output)
    );
}

#[test]
fn missing_extension_fails_before_running_tests() {
    let directory = tempfile::tempdir().unwrap();
    let test = directory.path().join("test.slt");
    fs::write(&test, "statement ok\nSELECT 1\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .arg("--extension")
        .arg(directory.path().join("missing.duckdb_extension"))
        .arg(test)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        !output_text(&output).contains("PASS"),
        "{}",
        output_text(&output)
    );
}

#[test]
fn disabled_sirius_cannot_report_a_pass() {
    let directory = tempfile::tempdir().unwrap();
    let extension = directory.path().join("invalid.duckdb_extension");
    let test = directory.path().join("test.slt");
    fs::write(&extension, "not an extension").unwrap();
    fs::write(&test, "statement ok\nSELECT 1\n").unwrap();
    for value in ["1", "", "false", "0"] {
        let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .env("SIRIUS_DISABLE", value)
            .arg("--extension")
            .arg(&extension)
            .arg(&test)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", output_text(&output));
        assert_eq!(
            output_text(&output).contains("unset SIRIUS_DISABLE"),
            value != "0",
            "{}",
            output_text(&output)
        );
    }
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .env("SIRIUS_DISABLE", "1")
        .arg(test)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
}

#[test]
#[ignore = "requires SIRIUS_SQLTEST_EXTENSION pointing to a compatible GPU extension"]
fn sirius_settings_and_examples() {
    let extension =
        std::env::var_os("SIRIUS_SQLTEST_EXTENSION").expect("set SIRIUS_SQLTEST_EXTENSION");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let directory = tempfile::tempdir().unwrap();
    let setup = directory.path().join("settings.slt");
    fs::write(&setup, "onlyif sirius\nquery TT\nSELECT current_setting('gpu_execution')::VARCHAR, current_setting('enable_duckdb_fallback')::VARCHAR\n----\ntrue\nfalse\n\nonlyif duckdb\nstatement error\nSELECT 1\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .arg("--extension")
        .arg(extension)
        .args([
            setup,
            root.join("test/sqltest/aggregate.slt"),
            root.join("test/sqltest/join.slt"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
}
