//! SQLLogicTest formatting without executing SQL.

use anyhow::{Context, Result, ensure};
use clap::Args;
use sqllogictest::{DefaultColumnType, QueryExpect, Record, StatementExpect};
use sqlparser::{
    dialect::DuckDbDialect,
    parser::Parser,
    tokenizer::{Token, Tokenizer, Whitespace},
};
use std::{collections::BTreeSet, fs, path::PathBuf};

#[derive(Args)]
pub struct FormatArgs {
    #[arg(default_values = ["test/sqltest/suites", "test/sqltest/fixtures"])]
    pub paths: Vec<PathBuf>,
    #[arg(long)]
    pub check: bool,
}

pub fn sql(input: &str) -> Result<String> {
    if input.contains("__RELATION__") {
        return Ok(input.to_owned());
    }
    let dialect = DuckDbDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, input).tokenize() else {
        return Ok(input.to_owned());
    };
    if tokens.iter().any(|t| {
        matches!(
            t,
            Token::Whitespace(
                Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_)
            )
        )
    }) {
        return Ok(input.to_owned());
    }
    let Ok(statements) = Parser::parse_sql(&dialect, input) else {
        return Ok(input.to_owned());
    };
    let formatted = statements
        .iter()
        .map(|s| format!("{s:#};"))
        .collect::<Vec<_>>()
        .join("\n");
    let formatted = formatted
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let Ok(reparsed) = Parser::parse_sql(&dialect, &formatted) else {
        return Ok(input.to_owned());
    };
    if !statements
        .iter()
        .map(ToString::to_string)
        .eq(reparsed.iter().map(ToString::to_string))
    {
        return Ok(input.to_owned());
    }
    Ok(formatted)
}

pub fn slt(source: &str, name: &str) -> Result<String> {
    let mut records = sqllogictest::parse_with_name::<DefaultColumnType>(source, name)?;
    for record in &mut records {
        match record {
            Record::Statement {
                sql: query,
                expected,
                ..
            } if !matches!(expected, StatementExpect::Error(_)) => *query = sql(query)?,
            Record::Query {
                sql: query,
                expected,
                ..
            } if !matches!(expected, QueryExpect::Error(_)) => *query = sql(query)?,
            _ => {}
        }
    }
    Ok(records
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>()
        .trim_end()
        .to_owned()
        + "\n")
}

pub fn execute(args: &FormatArgs) -> Result<bool> {
    let mut files = BTreeSet::new();
    for path in &args.paths {
        if path.is_file() {
            files.insert(path.clone());
            continue;
        }
        ensure!(path.is_dir(), "path does not exist: {}", path.display());
        for entry in walkdir::WalkDir::new(path) {
            let entry = entry?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e == "slt" || e == "sql")
            {
                files.insert(entry.into_path());
            }
        }
    }
    let mut changes = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path)?;
        let formatted = if path.extension().is_some_and(|e| e == "sql") {
            sql(&source).map(|mut s| {
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s
            })
        } else {
            slt(&source, &path.to_string_lossy())
        }
        .with_context(|| format!("format {}", path.display()))?;
        if source != formatted {
            changes.push((path, formatted));
        }
    }
    for (path, formatted) in &changes {
        println!("{}", path.display());
        if !args.check {
            fs::write(path, formatted)?;
        }
    }
    Ok(!args.check || changes.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_preserves_slt_metadata_and_expected_output() {
        let source = "include fixture.slt\n\n# sirius: id = \"test/quoted\"\n# sirius: snapshot = true\nquery TT rowsort\nselect 'NULL', 'a; b';\n----\n\"NULL\"\t\"a; b\"\n\nquery error syntax error\nSELECT FROM;\n";
        let formatted = slt(source, "example.slt").unwrap();
        assert!(formatted.contains("SELECT\n  'NULL',\n  'a; b';"));
        assert!(formatted.contains("# sirius: snapshot = true"));
        assert!(formatted.contains("\"NULL\"\t\"a; b\""));
        assert!(formatted.contains("query error syntax error\nSELECT FROM;"));
        assert_eq!(slt(&formatted, "example.slt").unwrap(), formatted);
    }

    #[test]
    fn preserves_comments_placeholders_and_unrecognized_duckdb_statements() {
        for input in [
            "SELECT 1 -- keep the reason\n;",
            "CREATE __RELATION__ t AS SELECT 1;",
            "SELECT 'CREATE __RELATION__ t';",
            "CHECKPOINT;",
            "SELECT 'line with trailing spaces  \nnext line';",
        ] {
            assert_eq!(sql(input).unwrap(), input);
        }
    }

    #[test]
    fn formatted_ddl_has_no_trailing_whitespace() {
        let formatted = sql("CREATE TABLE t (i INTEGER, s VARCHAR);").unwrap();
        assert!(formatted.lines().all(|line| line == line.trim_end()));
        assert_eq!(sql(&formatted).unwrap(), formatted);
    }
}
