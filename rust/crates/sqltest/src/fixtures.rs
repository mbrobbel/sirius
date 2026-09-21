use crate::{
    config::{Config, Name, PathBinding, QuerySource, Selection},
    corpus, worker,
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Args)]
pub struct PrepareArgs {
    #[arg(long, default_value = "test/sqltest")]
    pub root: PathBuf,
    #[arg(long, default_value = ".cache/sqltest/data")]
    pub output: PathBuf,
    #[arg(long)]
    pub run: Option<Name>,
    #[arg(long)]
    pub suite: Option<Selection>,
    #[arg(long, value_name = "NAME=PATH")]
    pub source: Vec<PathBinding>,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    runtime: String,
    datasets: BTreeMap<Name, Dataset>,
}
#[derive(Serialize, Deserialize)]
struct Dataset {
    recipe: String,
    files: BTreeMap<String, String>,
}

fn recipe(config: &Config, name: &Name) -> Result<String> {
    let fixture = &config.manifest.fixtures[name];
    let mut input = serde_json::to_vec(fixture)?;
    for path in &fixture.sql {
        input.extend_from_slice(&fs::read(config.path(path)?)?);
    }
    for source in &fixture.sources {
        input.extend_from_slice(&serde_json::to_vec(&config.manifest.sources[source])?);
    }
    Ok(corpus::hash(input))
}

pub fn verify(config: &Config, directory: &Path, required: &[Name]) -> Result<String> {
    if required.is_empty() {
        return Ok(corpus::hash("inline"));
    }
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(directory.join("manifest.json")).context("run prepare to provision fixtures")?,
    )?;
    ensure!(
        manifest.version == 2,
        "unsupported fixture manifest; prepare into a fresh output directory"
    );
    let mut identity = manifest.runtime.clone();
    for name in required {
        let dataset = manifest
            .datasets
            .get(name)
            .with_context(|| format!("fixture {name} is not prepared"))?;
        ensure!(
            dataset.recipe == recipe(config, name)?,
            "fixture {name} recipe changed; prepare into a fresh output directory"
        );
        let expected: BTreeSet<_> = config.manifest.fixtures[name]
            .tables
            .iter()
            .map(|table| format!("{name}/{table}.parquet"))
            .collect();
        ensure!(
            dataset.files.keys().cloned().collect::<BTreeSet<_>>() == expected,
            "fixture {name} file list differs from its recipe"
        );
        for (file, expected) in &dataset.files {
            ensure!(
                corpus::file_hash(&directory.join(file))? == *expected,
                "fixture checksum mismatch: {file}"
            );
        }
        identity.push_str(&serde_json::to_string(dataset)?);
    }
    Ok(corpus::hash(identity))
}

pub fn provision(con: &duckdb::Connection, extension: &Name) -> Result<()> {
    let name = worker::quote(extension.as_ref());
    if con.execute_batch(&format!("LOAD {name};")).is_err() {
        con.execute_batch(&format!("INSTALL {name}; LOAD {name};"))?;
    }
    Ok(())
}

pub fn prepare(args: &PrepareArgs) -> Result<()> {
    let config = Config::load(&args.root)?;
    let suites = config.suites(args.run.as_ref(), args.suite.as_ref())?;
    let required: BTreeSet<_> = suites
        .iter()
        .flat_map(|s| config.suites[s].fixtures.iter().cloned())
        .collect();
    let mut sources = BTreeMap::new();
    for binding in &args.source {
        ensure!(
            config.manifest.sources.contains_key(&binding.name),
            "unknown source {}",
            binding.name
        );
        ensure!(
            sources
                .insert(binding.name.clone(), &binding.path)
                .is_none(),
            "duplicate source {}",
            binding.name
        );
    }
    fs::create_dir_all(&args.output)?;
    let output = args.output.canonicalize()?;
    let runtime = corpus::file_hash(&crate::run::runtime_path()?)?;
    let mut manifest = if output.join("manifest.json").exists() {
        let existing: Manifest = serde_json::from_slice(&fs::read(output.join("manifest.json"))?)?;
        ensure!(
            existing.version == 2 && existing.runtime == runtime,
            "fixture cache belongs to a different runtime or format; choose a new --output"
        );
        existing
    } else {
        Manifest {
            version: 2,
            runtime,
            datasets: BTreeMap::new(),
        }
    };
    for name in required {
        if manifest.datasets.contains_key(&name) {
            verify(&config, &output, std::slice::from_ref(&name))?;
            eprintln!("{name}: using verified fixture cache");
            continue;
        }
        let fixture = &config.manifest.fixtures[&name];
        let mut replacements = Vec::new();
        for source in &fixture.sources {
            let spec = &config.manifest.sources[source];
            let path = sources.get(source).with_context(|| format!("fixture {name} needs --source {source}=PATH ({}; provide the decompressed file)", spec.url))?.canonicalize()?;
            ensure!(
                corpus::file_hash(&path)? == spec.sha256.as_ref(),
                "source {source} checksum mismatch"
            );
            replacements.push((
                format!("__SOURCE_{source}__"),
                path.to_string_lossy().replace('\'', "''"),
            ));
        }
        let con = worker::connect(None, true)?;
        for extension in &fixture.extensions {
            provision(&con, extension)?;
        }
        for file in &fixture.sql {
            let mut sql = fs::read_to_string(config.path(file)?)?;
            for (key, value) in &replacements {
                sql = sql.replace(key, value);
            }
            ensure!(
                !sql.contains("__SOURCE_"),
                "unbound source in {}",
                file.display()
            );
            con.execute_batch(&sql)
                .with_context(|| format!("fixture {name}: {}", file.display()))?;
        }
        let temporary = tempfile::tempdir_in(&output)?;
        let mut files = BTreeMap::new();
        for table in &fixture.tables {
            let file = temporary.path().join(format!("{table}.parquet"));
            con.execute_batch(&format!(
                "COPY \"{table}\" TO {} (FORMAT PARQUET, COMPRESSION ZSTD);",
                worker::quote(&file.to_string_lossy())
            ))?;
            files.insert(format!("{name}/{table}.parquet"), corpus::file_hash(&file)?);
        }
        let destination = output.join(name.as_ref());
        ensure!(
            !destination.exists(),
            "untracked fixture directory {}; choose a fresh --output",
            destination.display()
        );
        fs::rename(temporary.path(), destination)?;
        manifest.datasets.insert(
            name.clone(),
            Dataset {
                recipe: recipe(&config, &name)?,
                files,
            },
        );
        fs::write(
            output.join("manifest.json.tmp"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        fs::rename(
            output.join("manifest.json.tmp"),
            output.join("manifest.json"),
        )?;
        eprintln!("{name}: prepared fixture");
    }
    Ok(())
}

/// Preserve upstream ordering and add output-column tie breakers before LIMIT/OFFSET.
fn deterministic(sql: &str, columns: usize) -> Result<(String, bool)> {
    use sqlparser::{
        ast::{OrderByKind, Statement},
        dialect::DuckDbDialect,
        parser::Parser,
    };
    let mut statements = Parser::parse_sql(&DuckDbDialect {}, sql)?;
    ensure!(statements.len() == 1, "benchmark must contain one query");
    let Statement::Query(query) = &mut statements[0] else {
        anyhow::bail!("benchmark is not a query");
    };
    let ordered = query.order_by.is_some() || query.limit_clause.is_some();
    if !ordered {
        return Ok((sql.trim().trim_end_matches(';').into(), false));
    }
    let tie_sql = format!(
        "SELECT 1 ORDER BY {}",
        (1..=columns)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut tie = Parser::parse_sql(&DuckDbDialect {}, &tie_sql)?;
    let Statement::Query(tie) = &mut tie[0] else {
        unreachable!()
    };
    if let Some(order) = &mut query.order_by {
        if let (OrderByKind::Expressions(existing), OrderByKind::Expressions(ties)) =
            (&mut order.kind, tie.order_by.take().unwrap().kind)
        {
            existing.extend(ties);
        }
    } else {
        query.order_by = tie.order_by.take();
    }
    Ok((statements[0].to_string(), true))
}

#[derive(Default, Serialize, Deserialize)]
struct ImportLog {
    #[serde(default)]
    suites: BTreeMap<Name, ImportOrigin>,
    adaptation: String,
}

#[derive(Serialize, Deserialize)]
struct ImportOrigin {
    source: String,
    duckdb_version: String,
    runtime_sha256: String,
    queries_sha256: String,
}

pub fn import(root: &Path, selection: &Selection) -> Result<()> {
    let config = Config::load(root)?;
    let suites = config.suites(None, Some(selection))?;
    let provenance_path = root.join("provenance.json");
    let mut provenance: ImportLog = if provenance_path.exists() {
        serde_json::from_slice(&fs::read(&provenance_path)?)?
    } else {
        ImportLog::default()
    };
    let runtime_sha256 = corpus::file_hash(&crate::run::runtime_path()?)?;
    let mut imported = 0;
    for suite_name in suites {
        let suite = &config.suites[&suite_name];
        let Some(spec) = &suite.import else {
            continue;
        };
        let con = worker::connect(None, true)?;
        for extension in &spec.extensions {
            provision(&con, extension)?;
        }
        for path in &spec.schema {
            con.execute_batch(&fs::read_to_string(config.path(path)?)?)?;
        }
        let queries: Vec<String> = match &spec.source {
            QuerySource::Database { query } => con
                .prepare(query)?
                .query_map([], |r| r.get(0))?
                .collect::<duckdb::Result<_>>()?,
            QuerySource::SqlLines { path } => fs::read_to_string(config.path(path)?)?
                .lines()
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
                .collect(),
        };
        ensure!(
            queries.len() == spec.expected_queries.get(),
            "unexpected {suite_name} query count: {}",
            queries.len()
        );
        let fixture = suite.fixtures.iter().flat_map(|name| config.manifest.fixtures[name].tables.iter().map(move |table| (name, table))).map(|(name, table)| format!("statement ok\nCREATE __RELATION__ \"{table}\" AS SELECT * FROM read_parquet('__FIXTURE_ROOT__/{name}/{table}.parquet');\n")).collect::<Vec<_>>().join("\n");
        fs::create_dir_all(root.join("fixtures"))?;
        fs::write(root.join(format!("fixtures/{suite_name}.slt")), fixture)?;
        let suite_dir = &suite.directory;
        let relative = suite_dir
            .strip_prefix(&config.root)
            .context("cannot import into an external suite")?;
        let prefix = "../".repeat(relative.components().count());
        fs::create_dir_all(root.join(format!("upstream/{suite_name}")))?;
        for (i, sql) in queries.iter().enumerate() {
            let id = format!("{suite_name}/q{:02}", i + 1);
            let mut stmt = con.prepare(sql)?;
            let result = stmt.query_arrow([])?;
            let schema = result.get_schema();
            drop(result);
            let (query, ordered) =
                deterministic(sql, schema.fields().len()).with_context(|| id.clone())?;
            let mut meta =
                format!("# sirius: id = \"{id}\"\n# sirius: tags = [\"{suite_name}\"]\n");
            let tolerances: Vec<_> = schema
                .fields()
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    matches!(
                        f.data_type(),
                        arrow::datatypes::DataType::Float32 | arrow::datatypes::DataType::Float64
                    )
                })
                .map(|(i, _)| format!("\"{i}\" = {{ absolute = 1e-8, relative = 1e-9 }}"))
                .collect();
            if !tolerances.is_empty() {
                meta.push_str(&format!(
                    "# sirius: tolerances = {{ {} }}\n",
                    tolerances.join(", ")
                ));
            }
            let contents = format!(
                "# Source and license: {prefix}upstream/README.md\ninclude {prefix}fixtures/{suite_name}.slt\n\n{meta}\nquery {} {}\n{query};\n----\n",
                "?".repeat(schema.fields().len()),
                if ordered { "nosort" } else { "rowsort" }
            );
            fs::write(suite_dir.join(format!("q{:02}.slt", i + 1)), contents)?;
            fs::write(
                root.join(format!("upstream/{id}.sql")),
                format!("{}\n", sql.trim_end()),
            )?;
        }
        provenance.suites.insert(
            suite_name,
            ImportOrigin {
                source: spec.provenance.clone(),
                duckdb_version: con.query_row("SELECT version()", [], |r| r.get(0))?,
                runtime_sha256: runtime_sha256.clone(),
                queries_sha256: corpus::hash(serde_json::to_vec(&queries)?),
            },
        );
        imported += queries.len();
    }
    ensure!(imported > 0, "selected suites have no import definitions");
    provenance.adaptation = "Top-level ordered/limited queries append output-column ordinals as tie breakers. Original SQL is retained under upstream/.".into();
    fs::write(
        provenance_path,
        format!("{}\n", serde_json::to_string_pretty(&provenance)?),
    )?;
    Ok(())
}
