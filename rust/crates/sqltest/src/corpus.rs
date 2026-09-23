use crate::result::{Tolerance, Tolerances};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqllogictest::{
    Condition, Connection, DefaultColumnType, ExpectedError, QueryExpect, Record, SortMode,
    StatementExpect,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub fn hash(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}
pub fn file_hash(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut input = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Expected {
    Ok,
    Count(u64),
    Error(String),
}

impl Expected {
    pub fn error(error: ExpectedError) -> Result<Self> {
        Ok(Self::Error(match error {
            ExpectedError::Empty => ".*".into(),
            ExpectedError::Inline(regex) => regex.as_str().into(),
            ExpectedError::Multiline(s) => format!("(?s)^{}$", regex::escape(s.trim())),
            ExpectedError::SqlState(_) => bail!(
                "SQLSTATE assertions are not supported by the DuckDB adapter; use an error regex"
            ),
        }))
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    id: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    snapshot: bool,
    #[serde(default)]
    execution: Execution,
    timeout: Option<u64>,
    #[serde(default)]
    tolerances: BTreeMap<String, Tolerance>,
    float_tolerance: Option<Tolerance>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Execution {
    #[default]
    NoFallback,
    AllowFallback,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionId {
    #[default]
    Default,
    Named(String),
}

impl From<Connection> for ConnectionId {
    fn from(connection: Connection) -> Self {
        match connection {
            Connection::Default => Self::Default,
            Connection::Named(name) => Self::Named(name),
        }
    }
}

impl ConnectionId {
    pub fn label(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::Named(name) => name,
        }
    }
}

impl Execution {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoFallback => "fallback disabled",
            Self::AllowFallback => "fallback allowed",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Case {
    pub connection: ConnectionId,
    pub id: String,
    pub file: String,
    pub line: u32,
    pub sql: String,
    pub tags: Vec<String>,
    pub ordered: bool,
    pub timeout: u64,
    pub tolerances: Tolerances,
    pub float_tolerance: Option<Tolerance>,
    pub snapshot: bool,
    pub expected_rows: Vec<String>,
    pub expected_columns: usize,
    pub expected: Expected,
    pub execution: Execution,
}

impl Case {
    pub fn resolved_tolerances(&self, batch: &arrow::record_batch::RecordBatch) -> Tolerances {
        crate::result::resolve_tolerances(batch, &self.tolerances, self.float_tolerance)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Engine {
    DuckDb,
    Sirius,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Scope {
    Both,
    DuckDb,
    Sirius,
}

impl Scope {
    fn parse(conditions: Vec<Condition>) -> Result<Self> {
        let (mut duckdb, mut sirius) = (true, true);
        for condition in conditions {
            let (label, only) = match condition {
                Condition::OnlyIf { label } => (label, true),
                Condition::SkipIf { label } => (label, false),
            };
            ensure!(
                matches!(label.as_str(), "duckdb" | "sirius"),
                "unknown engine label {label:?}; use duckdb or sirius"
            );
            duckdb &= (label == "duckdb") == only;
            sirius &= (label == "sirius") == only;
        }
        match (duckdb, sirius) {
            (true, true) => Ok(Self::Both),
            (true, false) => Ok(Self::DuckDb),
            (false, true) => Ok(Self::Sirius),
            (false, false) => bail!("setup conditions exclude both engines"),
        }
    }

    pub fn applies(&self, engine: Engine) -> bool {
        matches!(
            (self, engine),
            (Self::Both, _) | (Self::DuckDb, Engine::DuckDb) | (Self::Sirius, Engine::Sirius)
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Step {
    Setup {
        connection: ConnectionId,
        sql: String,
        expected: Expected,
        scope: Scope,
    },
    Query(Case),
}

#[derive(Clone, Debug)]
pub struct Script {
    pub path: PathBuf,
    pub fingerprint: String,
    pub steps: Vec<Step>,
    pub expanded: String,
}

impl Script {
    pub fn has_named_connections(&self) -> bool {
        self.steps.iter().any(|step| {
            let connection = match step {
                Step::Setup { connection, .. } => connection,
                Step::Query(case) => &case.connection,
            };
            matches!(connection, ConnectionId::Named(_))
        })
    }
}

fn expand(
    path: &Path,
    stack: &mut Vec<PathBuf>,
    records: &mut Vec<Record<DefaultColumnType>>,
    inputs: &mut Vec<u8>,
) -> Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("include {}", path.display()))?;
    ensure!(
        !stack.contains(&path),
        "include cycle: {:?} -> {}",
        stack,
        path.display()
    );
    stack.push(path.clone());
    let source = fs::read_to_string(&path)?;
    inputs.extend_from_slice(source.as_bytes());
    let mut pending_condition = false;
    let mut pending_connection = false;
    for record in sqllogictest::parse_with_name::<DefaultColumnType>(
        &source,
        path.to_string_lossy().as_ref(),
    )? {
        match &record {
            Record::Condition(_) => pending_condition = true,
            Record::Connection(_) => pending_connection = true,
            Record::Statement { .. } | Record::Query { .. } => {
                pending_condition = false;
                pending_connection = false;
            }
            _ => {}
        }
        if let Record::Include { filename, .. } = record {
            ensure!(
                !pending_connection,
                "connection modifiers on includes are not supported"
            );
            ensure!(
                !pending_condition,
                "conditions on includes are not supported"
            );
            let pattern = path.parent().unwrap().join(filename);
            let mut files: Vec<_> =
                glob::glob(&pattern.to_string_lossy())?.collect::<std::result::Result<_, _>>()?;
            ensure!(
                !files.is_empty(),
                "include matched no files: {}",
                pattern.display()
            );
            files.sort();
            for file in files {
                expand(&file, stack, records, inputs)?;
            }
        } else {
            records.push(record);
        }
    }
    ensure!(
        !pending_condition,
        "condition without a statement in {}",
        path.display()
    );
    ensure!(
        !pending_connection,
        "connection without a statement in {}",
        path.display()
    );
    stack.pop();
    Ok(())
}

pub fn load(path: &Path, root: &Path) -> Result<Script> {
    let mut records = Vec::new();
    let mut inputs = Vec::new();
    expand(path, &mut Vec::new(), &mut records, &mut inputs)?;
    let expanded = records
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let mut steps = Vec::new();
    let mut metadata = String::new();
    for record in records {
        match record {
            Record::Comment(lines) => {
                for line in lines {
                    if let Some(value) = line.trim().strip_prefix("sirius:") {
                        metadata.push_str(value.trim());
                        metadata.push('\n');
                    }
                }
            }
            Record::Newline | Record::Connection(_) => {}
            Record::Condition(condition) => {
                Scope::parse(vec![condition])?;
            }
            Record::Statement {
                sql,
                expected,
                conditions,
                connection,
                retry,
                ..
            } => {
                ensure!(retry.is_none(), "retry records are not supported");
                ensure!(
                    metadata.trim().is_empty(),
                    "Sirius metadata must precede a query"
                );
                let expected = match expected {
                    StatementExpect::Ok => Expected::Ok,
                    StatementExpect::Count(n) => Expected::Count(n),
                    StatementExpect::Error(e) => Expected::error(e)?,
                };
                steps.push(Step::Setup {
                    connection: connection.into(),
                    sql,
                    expected,
                    scope: Scope::parse(conditions)?,
                });
            }
            Record::Query {
                sql,
                loc,
                expected,
                conditions,
                connection,
                retry,
            } => {
                ensure!(
                    conditions.is_empty() && retry.is_none(),
                    "conditional queries and retry records are not supported"
                );
                let meta: Metadata =
                    toml::from_str(&metadata).with_context(|| format!("metadata at {loc}"))?;
                metadata.clear();
                let id = meta
                    .id
                    .context("each query requires '# sirius: id = \"suite/name\"'")?;
                ensure!(
                    !id.is_empty()
                        && id
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || "/_-".contains(c)),
                    "invalid case ID {id:?}"
                );
                let timeout = meta.timeout.unwrap_or(120);
                ensure!(timeout > 0, "timeout must be positive");
                let mut tolerances = Tolerances::new();
                if let Some(tolerance) = meta.float_tolerance {
                    tolerance.validate()?;
                }
                for (column, tolerance) in meta.tolerances {
                    tolerance.validate()?;
                    tolerances.insert(
                        column
                            .parse()
                            .context("tolerance keys must be zero-based column indices")?,
                        tolerance,
                    );
                }
                let (ordered, expected_rows, expected_columns, expected) = match expected {
                    QueryExpect::Results {
                        sort_mode,
                        result_mode,
                        label,
                        types,
                        results,
                    } => {
                        ensure!(
                            result_mode.is_none() && label.is_none(),
                            "result modes and query labels are not supported"
                        );
                        ensure!(
                            sort_mode != Some(SortMode::ValueSort),
                            "valuesort loses row relationships; use rowsort or nosort"
                        );
                        (
                            sort_mode != Some(SortMode::RowSort),
                            results,
                            types.len(),
                            Expected::Ok,
                        )
                    }
                    QueryExpect::Error(e) => (true, Vec::new(), 0, Expected::error(e)?),
                };
                let source = Path::new(loc.file());
                let file = source
                    .strip_prefix(root)
                    .unwrap_or(source)
                    .to_string_lossy()
                    .into_owned();
                steps.push(Step::Query(Case {
                    connection: connection.into(),
                    id,
                    file,
                    line: loc.line(),
                    sql,
                    tags: meta.tags,
                    ordered,
                    timeout,
                    tolerances,
                    float_tolerance: meta.float_tolerance,
                    snapshot: meta.snapshot,
                    expected_rows,
                    expected_columns,
                    expected,
                    execution: meta.execution,
                }));
            }
            other => bail!("unsupported SLT record: {other:?}"),
        }
    }
    ensure!(metadata.trim().is_empty(), "metadata without a query");
    Ok(Script {
        expanded,
        path: path.to_path_buf(),
        fingerprint: hash(inputs),
        steps,
    })
}

pub fn discover(root: &Path, directory: &Path) -> Result<Vec<Script>> {
    let root = root.canonicalize()?;
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(directory) {
        let entry = entry?;
        if entry.file_type().is_file() && entry.path().extension().is_some_and(|s| s == "slt") {
            files.push(entry.into_path());
        }
    }
    files.sort();
    ensure!(!files.is_empty(), "no tests in {}", directory.display());
    let scripts: Vec<_> = files
        .iter()
        .map(|p| load(p, &root))
        .collect::<Result<_>>()?;
    let mut ids = std::collections::HashSet::new();
    for script in &scripts {
        for step in &script.steps {
            if let Step::Query(case) = step {
                ensure!(ids.insert(&case.id), "duplicate case ID {}", case.id);
            }
        }
    }
    Ok(scripts)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn setup_conditions_require_known_nonempty_engine_scope() {
        for (record, duckdb, sirius) in [
            ("", true, true),
            ("onlyif duckdb\n", true, false),
            ("skipif sirius\n", true, false),
            ("onlyif sirius\n", false, true),
            ("skipif duckdb\n", false, true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("scope.slt");
            fs::write(&file, format!("{record}statement ok\nSET threads = 1;\n")).unwrap();
            let script = load(&file, dir.path()).unwrap();
            let Step::Setup { scope, .. } = &script.steps[0] else {
                panic!("expected setup")
            };
            assert_eq!(scope.applies(Engine::DuckDb), duckdb);
            assert_eq!(scope.applies(Engine::Sirius), sirius);
        }
        for record in ["onlyif typo\n", "onlyif duckdb\nonlyif sirius\n"] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("invalid.slt");
            fs::write(&file, format!("{record}statement ok\nSELECT 1;\n")).unwrap();
            assert!(load(&file, dir.path()).is_err());
        }
    }

    #[test]
    fn record_modifiers_cannot_leak_across_include_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("main.slt");
        let fixture = dir.path().join("fixture.slt");
        for (modifier, include_error, dangling_error) in [
            (
                "onlyif sirius",
                "conditions on includes",
                "condition without a statement",
            ),
            (
                "connection reader",
                "connection modifiers on includes",
                "connection without a statement",
            ),
        ] {
            fs::write(&fixture, "statement ok\nCREATE TABLE t (i INTEGER);\n").unwrap();
            fs::write(&file, format!("{modifier}\ninclude fixture.slt\n")).unwrap();
            assert!(
                load(&file, dir.path())
                    .unwrap_err()
                    .to_string()
                    .contains(include_error)
            );

            fs::write(&fixture, format!("{modifier}\n")).unwrap();
            fs::write(&file, "include fixture.slt\n\nstatement ok\nSELECT 1;\n").unwrap();
            assert!(
                load(&file, dir.path())
                    .unwrap_err()
                    .to_string()
                    .contains(dangling_error)
            );
        }
    }

    #[test]
    fn includes_setup_and_detects_cycles() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join("fixture.slt"),
            "statement ok\nCREATE TABLE t(i INTEGER);\n",
        )?;
        let file = dir.path().join("test.slt");
        fs::write(
            &file,
            "include fixture.slt\n\n# sirius: id = \"test/a\"\nquery I rowsort\nSELECT * FROM t;\n----\n",
        )?;
        let script = load(&file, dir.path())?;
        assert_eq!(script.steps.len(), 2);
        fs::write(dir.path().join("fixture.slt"), "include test.slt\n")?;
        assert!(
            load(&file, dir.path())
                .unwrap_err()
                .to_string()
                .contains("cycle")
        );
        Ok(())
    }
}
