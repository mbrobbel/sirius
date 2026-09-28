use crate::{config::Relation, corpus::Engine};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::LazyLock};

static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"__[A-Z][A-Z0-9_]*?__").unwrap());
const BUILTINS: [&str; 4] = ["FIXTURE_ROOT", "TEST_DIR", "RELATION", "S3_BUCKET"];

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);

impl TryFrom<String> for Name {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            value.starts_with(|c: char| c.is_ascii_uppercase())
                && value
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && !value.contains("__")
                && !value.ends_with('_'),
            "invalid substitution name {value:?}; use uppercase letters, digits and single underscores"
        );
        ensure!(
            !BUILTINS.contains(&value.as_str()),
            "reserved substitution name {value}"
        );
        Ok(Self(value))
    }
}
impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}
impl From<Name> for String {
    fn from(value: Name) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Template(String);

impl TryFrom<String> for Template {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(!value.contains('\0'), "substitution contains NUL");
        for token in TOKEN.find_iter(&value) {
            ensure!(
                matches!(
                    token.as_str(),
                    "__FIXTURE_ROOT__" | "__TEST_DIR__" | "__S3_BUCKET__"
                ),
                "substitution values may only reference FIXTURE_ROOT, TEST_DIR or S3_BUCKET; found {}",
                token.as_str()
            );
        }
        Ok(Self(value))
    }
}
impl From<Template> for String {
    fn from(value: Template) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Values {
    duckdb: Template,
    sirius: Template,
}
pub type Substitutions = BTreeMap<Name, Values>;

pub fn validate(values: &Substitutions, has_object_store: bool) -> Result<()> {
    for value in values.values() {
        for template in [&value.duckdb, &value.sirius] {
            ensure!(
                has_object_store || !template.0.contains("__S3_BUCKET__"),
                "S3_BUCKET substitution requires a suite object_store"
            );
        }
    }
    Ok(())
}

fn replace(input: &str, mut resolve: impl FnMut(&str) -> Result<String>) -> Result<String> {
    let mut output = String::new();
    let mut end = 0;
    for token in TOKEN.find_iter(input) {
        output.push_str(&input[end..token.start()]);
        output.push_str(&resolve(token.as_str())?);
        end = token.end();
    }
    output.push_str(&input[end..]);
    Ok(output)
}

pub struct ContextValues<'a> {
    pub fixtures: &'a Path,
    pub relation: Relation,
    pub scratch: &'a Path,
    pub bucket: Option<&'a str>,
}
impl ContextValues<'_> {
    fn builtin(&self, token: &str) -> Result<String> {
        Ok(match token {
            "__FIXTURE_ROOT__" => self.fixtures.to_string_lossy().into_owned(),
            "__TEST_DIR__" => self.scratch.to_string_lossy().into_owned(),
            "__RELATION__" => self.relation.sql().to_owned(),
            "__S3_BUCKET__" => self
                .bucket
                .context("S3_BUCKET requires a suite object_store")?
                .to_owned(),
            _ => anyhow::bail!("unknown SQL substitution {token}"),
        })
    }

    pub fn sql(&self, sql: &str, values: &Substitutions, engine: Engine) -> Result<String> {
        replace(sql, |token| {
            let name = &token[2..token.len() - 2];
            let value = if let Some(value) = values.get(name) {
                let template = match engine {
                    Engine::DuckDb => &value.duckdb,
                    Engine::Sirius => &value.sirius,
                };
                replace(&template.0, |builtin| self.builtin(builtin))?
            } else {
                self.builtin(token)?
            };
            Ok(value.replace('\'', "''"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_paths_expand_once_for_each_engine() {
        let values: Substitutions = toml::from_str(
            r#"
[OBJECT_ROOT]
duckdb = "__FIXTURE_ROOT__/it's data"
sirius = "s3://__S3_BUCKET__"
"#,
        )
        .unwrap();
        let context = ContextValues {
            fixtures: Path::new("/tmp/it's/__TEST_DIR__"),
            relation: Relation::Table,
            scratch: Path::new("/private"),
            bucket: Some("sirius-test"),
        };
        let sql = "SELECT * FROM read_parquet('__OBJECT_ROOT__/file.parquet');";
        assert_eq!(
            context.sql(sql, &values, Engine::DuckDb).unwrap(),
            "SELECT * FROM read_parquet('/tmp/it''s/__TEST_DIR__/it''s data/file.parquet');"
        );
        assert_eq!(
            context.sql(sql, &values, Engine::Sirius).unwrap(),
            "SELECT * FROM read_parquet('s3://sirius-test/file.parquet');"
        );
        assert!(
            context
                .sql("SELECT '__MISSING__'", &values, Engine::DuckDb)
                .is_err()
        );
    }

    #[test]
    fn invalid_definitions_fail_at_parse_time() {
        for name in [
            "FIXTURE_ROOT",
            "TEST_DIR",
            "RELATION",
            "S3_BUCKET",
            "lowercase",
            "A__B",
            "A_",
        ] {
            assert!(Name::try_from(name.to_owned()).is_err(), "{name}");
        }
        for value in ["__CUSTOM__", "__RELATION__", "bad\0value"] {
            assert!(Template::try_from(value.to_owned()).is_err(), "{value:?}");
        }
        assert!(toml::from_str::<Substitutions>("[ROOT]\nduckdb = 'local'\n").is_err());
        let values =
            toml::from_str("[ROOT]\nduckdb = 'local'\nsirius = '__S3_BUCKET__'\n").unwrap();
        assert!(validate(&values, false).is_err());
        assert!(validate(&values, true).is_ok());
    }
}
