use crate::result::{Tolerance, Tolerances};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqllogictest::{
    Connection, DefaultColumnType, ExpectedError, QueryExpect, Record, SortMode, StatementExpect,
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
    timeout: Option<u64>,
    #[serde(default)]
    tolerances: BTreeMap<String, Tolerance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub file: String,
    pub line: u32,
    pub sql: String,
    pub tags: Vec<String>,
    pub ordered: bool,
    pub timeout: u64,
    pub tolerances: Tolerances,
    pub snapshot: bool,
    pub expected_rows: Vec<String>,
    pub expected_columns: usize,
    pub expected: Expected,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Step {
    Setup { sql: String, expected: Expected },
    Query(Case),
}

#[derive(Clone, Debug)]
pub struct Script {
    pub path: PathBuf,
    pub fingerprint: String,
    pub steps: Vec<Step>,
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
    for record in sqllogictest::parse_with_name::<DefaultColumnType>(
        &source,
        path.to_string_lossy().as_ref(),
    )? {
        if let Record::Include { filename, .. } = record {
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
    stack.pop();
    Ok(())
}

pub fn load(path: &Path, root: &Path) -> Result<Script> {
    let mut records = Vec::new();
    let mut inputs = Vec::new();
    expand(path, &mut Vec::new(), &mut records, &mut inputs)?;
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
            Record::Newline => {}
            Record::Statement {
                sql,
                expected,
                conditions,
                connection,
                retry,
                ..
            } => {
                ensure!(
                    conditions.is_empty() && connection == Connection::Default && retry.is_none(),
                    "conditional, multi-connection, and retry records are not supported"
                );
                ensure!(
                    metadata.trim().is_empty(),
                    "Sirius metadata must precede a query"
                );
                let expected = match expected {
                    StatementExpect::Ok => Expected::Ok,
                    StatementExpect::Count(n) => Expected::Count(n),
                    StatementExpect::Error(e) => Expected::error(e)?,
                };
                steps.push(Step::Setup { sql, expected });
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
                    conditions.is_empty() && connection == Connection::Default && retry.is_none(),
                    "conditional, multi-connection, and retry records are not supported"
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
                    id,
                    file,
                    line: loc.line(),
                    sql,
                    tags: meta.tags,
                    ordered,
                    timeout,
                    tolerances,
                    snapshot: meta.snapshot,
                    expected_rows,
                    expected_columns,
                    expected,
                }));
            }
            other => bail!("unsupported SLT record: {other:?}"),
        }
    }
    ensure!(metadata.trim().is_empty(), "metadata without a query");
    Ok(Script {
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
