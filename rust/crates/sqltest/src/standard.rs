use anyhow::{Result, bail, ensure};
use md5::{Digest, Md5};
use sqllogictest::{Connection, ExpectedError, Normalizer, QueryExpect, Record, StatementExpect};
use std::{collections::HashMap, fs, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Integer,
    Real,
    Text,
}

impl sqllogictest::ColumnType for Column {
    fn from_char(value: char) -> Option<Self> {
        match value {
            'I' => Some(Self::Integer),
            'R' => Some(Self::Real),
            'T' => Some(Self::Text),
            _ => None,
        }
    }

    fn to_char(&self) -> char {
        match self {
            Self::Integer => 'I',
            Self::Real => 'R',
            Self::Text => 'T',
        }
    }
}

/// A script restricted to the original SQLLogicTest record types.
pub struct Script(Vec<Record<Column>>);

impl Script {
    pub fn read(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        ensure!(
            text.is_ascii(),
            "SQLLogicTest scripts must be ASCII: {}",
            path.display()
        );
        let mut retry_label = "__slt_retry_label__".to_owned();
        while text.contains(&retry_label) {
            retry_label.push('_');
        }
        let text = original_headers(&text, &retry_label);
        // Do not use parse_file: it expands RisingLight's include directive.
        let mut records = sqllogictest::parse_with_name(&text, path.display().to_string())?;
        let mut labels = HashMap::new();
        let mut length = records.len();
        for (index, record) in records.iter_mut().enumerate() {
            match record {
                Record::Statement {
                    sql,
                    expected: StatementExpect::Ok | StatementExpect::Error(ExpectedError::Empty),
                    connection: Connection::Default,
                    retry: None,
                    ..
                } => {
                    *sql = without_comments(sql);
                }
                Record::Query {
                    sql,
                    expected:
                        QueryExpect::Results {
                            types,
                            label,
                            results,
                            result_mode: None,
                            ..
                        },
                    connection: Connection::Default,
                    retry: None,
                    loc,
                    ..
                } if !types.is_empty() => {
                    *sql = without_comments(sql);
                    results.retain(|line| !line.starts_with('#'));
                    if let Some(label) = label {
                        if label == &retry_label {
                            *label = "retry".to_owned();
                        }
                        let digest = expected_fingerprint(results);
                        if let Some(previous) = labels.insert(label.clone(), digest.clone()) {
                            ensure!(
                                previous == digest,
                                "query label {label:?} has different results at {loc}"
                            );
                        }
                    }
                }
                Record::Halt { .. } => {
                    length = index + 1;
                    break;
                }
                Record::HashThreshold { .. }
                | Record::Condition(_)
                | Record::Comment(_)
                | Record::Newline => {}
                _ => bail!(
                    "non-standard SQLLogicTest record in {}: {record:?}",
                    path.display()
                ),
            }
        }
        records.truncate(length);
        // Hash thresholds control completion output. During verification keep
        // actual rows intact so hash-shaped SQL text cannot impersonate a digest.
        records.retain(|record| !matches!(record, Record::HashThreshold { .. }));
        Ok(Self(records))
    }

    pub fn records(self) -> Vec<Record<Column>> {
        self.0
    }
}

// Preserve line numbers while allowing the ordinary label "retry" and stopping
// at halt before the library parses any subsequent records.
fn original_headers(text: &str, retry_label: &str) -> String {
    let mut output = String::new();
    let mut header = true;
    for line in text.lines() {
        if line.starts_with('#') {
            output.push_str(line);
            output.push('\n');
            continue;
        }
        let tokens: Vec<_> = line.split_whitespace().collect();
        if header && tokens == ["halt"] {
            output.push_str("halt\n");
            break;
        }
        let retry_is_label = header
            && matches!(
                tokens.as_slice(),
                ["query", _, "retry"] | ["query", _, "nosort" | "rowsort" | "valuesort", "retry"]
            );
        if retry_is_label {
            output.push_str(&tokens[..tokens.len() - 1].join(" "));
            output.push(' ');
            output.push_str(retry_label);
        } else {
            output.push_str(line);
        }
        output.push('\n');
        header =
            line.is_empty() || (header && matches!(tokens.first(), Some(&"onlyif" | &"skipif")));
    }
    output
}

fn without_comments(sql: &str) -> String {
    sql.lines()
        .filter(|line| !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn hash_line(line: &str) -> Option<(usize, String)> {
    let (count, digest) = line.split_once(" values hashing to ")?;
    if digest.len() != 32 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((count.parse().ok()?, digest.to_ascii_lowercase()))
}

fn expected_fingerprint(values: &[String]) -> (usize, String) {
    if values.len() == 1
        && let Some(hash) = hash_line(&values[0])
    {
        return hash;
    }
    fingerprint(values.iter().map(String::as_str))
}

fn fingerprint<'a>(values: impl IntoIterator<Item = &'a str>) -> (usize, String) {
    let mut count = 0;
    let mut hash = Md5::new();
    for value in values {
        count += 1;
        hash.update(value.as_bytes());
        hash.update(b"\n");
    }
    (count, format!("{:x}", hash.finalize()))
}

/// Original SLT stores one value per line and preserves printable whitespace.
pub fn compare(_: Normalizer, actual: &[Vec<String>], expected: &[String]) -> bool {
    let actual: Vec<_> = actual.iter().flatten().map(String::as_str).collect();
    if expected.len() == 1
        && let Some(hash) = hash_line(&expected[0])
    {
        fingerprint(actual) == hash
    } else {
        actual
            .iter()
            .copied()
            .eq(expected.iter().map(String::as_str))
    }
}
