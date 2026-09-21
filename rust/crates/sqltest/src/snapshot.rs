use crate::{
    corpus::{self, Expected, Step},
    result, run,
    worker::{Response, Worker},
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use sqllogictest::{DefaultColumnType, Record, RecordOutput};
use std::{collections::BTreeMap, fs, path::PathBuf};

#[derive(Args)]
pub struct CompleteArgs {
    pub file: PathBuf,
    #[arg(long, default_value = "test/sqltest")]
    pub root: PathBuf,
    #[arg(long, default_value = ".cache/sqltest/data")]
    pub fixtures: PathBuf,
}

#[allow(clippy::ptr_arg)]
fn unchanged(s: &String) -> String {
    s.clone()
}
fn exact(_: sqllogictest::Normalizer, actual: &[Vec<String>], expected: &[String]) -> bool {
    actual
        .iter()
        .map(|row| row.join("\t"))
        .eq(expected.iter().cloned())
}

pub fn complete(args: &CompleteArgs) -> Result<()> {
    let root = args.root.canonicalize()?;
    let file = args.file.canonicalize()?;
    let script = corpus::load(&file, &root)?;
    let fixtures = std::path::absolute(&args.fixtures)?;
    let directory = tempfile::tempdir()?;
    let mut worker = Worker::start(directory.path(), None, None)?;
    let mut outputs = BTreeMap::new();
    for step in &script.steps {
        match step {
            Step::Setup { sql, expected } => {
                run::assert_statement(
                    worker.execute(
                        &run::substitute(
                            sql,
                            &fixtures,
                            crate::config::Relation::Table,
                            directory.path(),
                        ),
                        false,
                        &directory.path().join("unused.arrow"),
                        300,
                    )?,
                    expected,
                )?;
            }
            Step::Query(case) => {
                let path = directory.path().join("result.arrow");
                let response = worker.execute(
                    &run::substitute(
                        &case.sql,
                        &fixtures,
                        crate::config::Relation::Table,
                        directory.path(),
                    ),
                    true,
                    &path,
                    case.timeout,
                )?;
                match response {
                    Response::Rows { .. } => {
                        ensure!(
                            !matches!(case.expected, Expected::Error(_)),
                            "reference unexpectedly succeeded for {}",
                            case.id
                        );
                        let batch = result::read(&path)?;
                        result::validate_alignment(&batch, case.ordered, &case.tolerances)?;
                        let rows = result::snapshot(&batch, case.ordered)?
                            .into_iter()
                            .map(|r| r.split('\t').map(str::to_owned).collect())
                            .collect();
                        outputs.insert(
                            (case.file.clone(), case.line),
                            RecordOutput::Query {
                                types: vec![DefaultColumnType::Any; batch.num_columns()],
                                rows,
                                error: None,
                            },
                        );
                    }
                    Response::Error {
                        message,
                        harness: false,
                    } if matches!(&case.expected, Expected::Error(p) if regex::Regex::new(p).unwrap().is_match(message.trim())) =>
                        {}
                    other => anyhow::bail!("cannot complete {}: {other:?}", case.id),
                }
            }
        }
    }
    let source = fs::read_to_string(&file)?;
    let records = sqllogictest::parse_with_name::<DefaultColumnType>(
        &source,
        file.to_string_lossy().as_ref(),
    )?;
    let mut completed = String::new();
    let mut metadata = String::new();
    for record in records {
        if let Record::Comment(lines) = record {
            let mut comments = Vec::new();
            for line in lines {
                if let Some(value) = line.trim().strip_prefix("sirius:") {
                    metadata.push_str(value.trim());
                    metadata.push('\n');
                } else {
                    comments.push(line);
                }
            }
            if !comments.is_empty() {
                completed.push_str(&Record::<DefaultColumnType>::Comment(comments).to_string());
                completed.push('\n');
            }
            continue;
        }
        if let Record::Query { loc, .. } = &record {
            let key = (
                file.strip_prefix(&root)
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .into_owned(),
                loc.line(),
            );
            let mut fields: toml::Table = toml::from_str(&metadata)?;
            if outputs.contains_key(&key) {
                fields.insert("snapshot".into(), toml::Value::Boolean(true));
            }
            for line in toml::to_string(&fields)?.lines() {
                completed.push_str(&format!("# sirius: {line}\n"));
            }
            metadata.clear();
            if let Some(output) = outputs.get(&key) {
                let replacement = sqllogictest::update_record_with_output(
                    &record,
                    output,
                    "\t",
                    exact,
                    unchanged,
                    sqllogictest::strict_column_validator,
                );
                completed.push_str(&replacement.as_ref().unwrap_or(&record).to_string());
                completed.push('\n');
                continue;
            }
        }
        completed.push_str(&record.to_string());
        completed.push('\n');
    }
    let mut temporary =
        tempfile::NamedTempFile::new_in(file.parent().context("file has no parent")?)?;
    std::io::Write::write_all(&mut temporary, completed.as_bytes())?;
    temporary.persist(&file)?;
    eprintln!(
        "completed {} from DuckDB; included files were not rewritten",
        file.display()
    );
    Ok(())
}
