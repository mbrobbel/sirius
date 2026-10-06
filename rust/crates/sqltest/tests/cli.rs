use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

fn run(files: &[(&str, &str)], selected: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    for (name, contents) in files {
        fs::write(directory.path().join(name), contents).unwrap();
    }
    Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args(selected.iter().map(|name| directory.path().join(name)))
        .output()
        .unwrap()
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn passes(script: &str) {
    let output = run(&[("test.slt", script)], &["test.slt"]);
    assert!(output.status.success(), "{}", output_text(&output));
}

fn fails(script: &str, reason: &str) {
    let output = run(&[("test.slt", script)], &["test.slt"]);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert!(
        output_text(&output).contains(reason),
        "{}",
        output_text(&output)
    );
    assert!(output_text(&output).contains("test.slt"));
}

#[test]
fn checked_in_tests_pass() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
        .args([
            root.join("test/sqltest/aggregate.slt"),
            root.join("test/sqltest/join.slt"),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", output_text(&output));
}

#[test]
fn original_value_layout_rounding_and_text() {
    passes("query R\nSELECT 1.2345::REAL\n----\n1.235\n");
    passes(
        "query I\nSELECT 340282366920938463463374607431768211455::UHUGEINT\n----\n340282366920938463463374607431768211455\n",
    );
    passes(
        "query IRRTTTT\nSELECT 42, 1.23456::DOUBLE, -1.23456::REAL, ' x  y ', 'a' || chr(9) || 'b', '', NULL::VARCHAR\n----\n42\n1.235\n-1.235\n x  y \na@b\n(empty)\nNULL\n",
    );
    passes("query RR\nSELECT 1.25::DECIMAL(8,2), 12::DECIMAL(8,0)\n----\n1.250\n12.000\n");
    fails(
        "query II\nSELECT 1, 2\n----\n1 2\n",
        "query result mismatch",
    );
    fails(
        "query R\nSELECT 1.25::DOUBLE\n----\n1.25\n",
        "query result mismatch",
    );
    fails(
        "query T\nSELECT 'x  y'\n----\nx y\n",
        "query result mismatch",
    );
    fails(
        "query T\nSELECT 'actual'\n----\n<slt:ignore>\n",
        "query result mismatch",
    );
}

#[test]
fn sorting_conditions_and_comments() {
    passes(
        r#"statement ok
CREATE TABLE t (n INTEGER)

statement ok
INSERT INTO t VALUES (3), (1), (2)

query I rowsort
SELECT n
# A comment inside SQL is ignored.
FROM t
----
1
# A comment among expected values is ignored.
2
3

query II valuesort
SELECT n, n + 1 FROM t WHERE n < 3
----
1
2
2
3

query I nosort
SELECT n FROM t ORDER BY n DESC
----
3
2
1

statement error
INSERT INTO missing_table VALUES (1)

onlyif duckdb
statement ok
CREATE TABLE selected (n INTEGER)

skipif other_engine
query I
SELECT COUNT(*) FROM selected
----
0

onlyif other_engine
statement error
SELECT 1

skipif duckdb
statement error
SELECT 1

halt

statement error
SELECT 1
"#,
    );
}

#[test]
fn hashes_and_labels() {
    fails(
        "query T\nSELECT '1 values hashing to 401b30e3b8b5d629635a5c613cdb7919'\n----\nx\n",
        "query result mismatch",
    );
    fails(
        "hash-threshold 1\n\nquery T\nSELECT '1 values hashing to 401b30e3b8b5d629635a5c613cdb7919'\n----\n1 values hashing to 401b30e3b8b5d629635a5c613cdb7919\n",
        "query result mismatch",
    );
    passes(
        "query I nosort retry\nSELECT 1\n----\n1\n\nonlyif duckdb\nquery I retry\nSELECT 1\n----\n1\n",
    );
    fails(
        "query I nosort retry\nSELECT 1\n----\n1\n\nquery I nosort retry\nSELECT 2\n----\n2\n",
        "query label",
    );
    passes(
        r#"query I rowsort equivalent
SELECT n FROM (VALUES (2), (1)) AS t(n)
----
1
2

hash-threshold 1

query I rowsort equivalent
SELECT n FROM (VALUES (1), (2)) AS t(n)
----
2 values hashing to 6ddb4095eb719e2a9f0a3f95677d24e0

hash-threshold 0

query II nosort equivalent
SELECT 1, 2
----
2 values hashing to 6ddb4095eb719e2a9f0a3f95677d24e0

onlyif other_engine
query I rowsort equivalent
SELECT * FROM missing_table
----
1
2
"#,
    );
    fails(
        "query I nosort same\nSELECT 1\n----\n1\n\nquery I nosort same\nSELECT 2\n----\n2\n",
        "query label",
    );
    fails(
        "query I nosort same\nSELECT 1\n----\n1\n\nskipif duckdb\nquery I nosort same\nSELECT 2\n----\n2\n",
        "query label",
    );
    fails(
        "query I\nSELECT 1\n----\n2 values hashing to 6ddb4095eb719e2a9f0a3f95677d24e0\n",
        "query result mismatch",
    );
}

#[test]
fn rejects_extensions_before_executing_sql() {
    for record in [
        "include nonexistent.slt",
        "connection other",
        "sleep 1s",
        "subtest name",
        "control substitution on",
        "control resultmode valuewise",
        "let x\nSELECT 1",
        "system ok\nprintf must_not_execute",
        "statement count 1\nSELECT 1",
        "statement ok retry 2 backoff 1s\nSELECT 1",
        "query error\nSELECT 1",
        "statement error missing_table\nSELECT * FROM missing_table",
    ] {
        let script = format!("statement ok\nSELECT * FROM must_not_execute\n\n{record}\n");
        let output = run(&[("test.slt", &script)], &["test.slt"]);
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            output_text(&output).contains("non-standard SQLLogicTest"),
            "{}",
            output_text(&output)
        );
        // The unsupported record is caught before the earlier SQL error.
        assert!(!output_text(&output).contains("Catalog Error"));
    }
    fails("query B\nSELECT true\n----\ntrue\n", "invalid type");
}

#[test]
fn validates_all_batches_and_empty_result_types() {
    passes("query T\nSELECT 'halt'\n----\nhalt\n\nhalt\n\nthis is not a record\n");
    let mut script = String::from("query I\nSELECT range FROM range(5000) ORDER BY range\n----\n");
    for n in 0..5000 {
        script.push_str(&format!("{n}\n"));
    }
    passes(&script);
    passes("query I\nSELECT 1 WHERE false\n----\n");
    fails(
        "query T\nSELECT 1 WHERE false\n----\n",
        "query columns mismatch",
    );
    fails(
        "statement error\nCREATE TABLE t (i INTEGER)\n",
        "expected to fail, but actually succeed",
    );
}

#[test]
fn files_are_isolated_and_continue_after_failure() {
    let output = run(
        &[
            (
                "first.slt",
                "statement ok\nCREATE TABLE t (i INTEGER)\n\nquery I\nSELECT 1\n----\n2\n",
            ),
            ("second.slt", "statement ok\nCREATE TABLE t (i INTEGER)\n"),
        ],
        &["first.slt", "second.slt"],
    );
    assert!(!output.status.success());
    assert!(
        output_text(&output).contains("1 passed; 1 failed"),
        "{}",
        output_text(&output)
    );
}

#[test]
fn missing_inputs_and_unknown_options_fail() {
    for args in [vec![], vec!["missing.slt"], vec!["--unknown-option"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_sirius-sqltest"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", output_text(&output));
    }
}

#[test]
fn pinned_runtime_version() {
    passes("query T\nSELECT version()\n----\nv1.5.6\n");
}
