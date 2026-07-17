//! DataFusion SQL planning and `NamedTable` to Parquet `LocalFiles` binding.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use datafusion::common::TableReference;
use datafusion::datasource::listing::{ListingTable, ListingTableUrl};
use datafusion::error::{DataFusionError, Result};
use datafusion::prelude::SessionContext;
use datafusion_substrait::logical_plan::producer::to_substrait_plan;
use datafusion_substrait::substrait::proto::read_rel::local_files::FileOrFiles;
use datafusion_substrait::substrait::proto::read_rel::local_files::file_or_files::{
    FileFormat, ParquetReadOptions, PathType,
};
use datafusion_substrait::substrait::proto::read_rel::{LocalFiles, ReadType};
use datafusion_substrait::substrait::proto::rel::RelType;
use datafusion_substrait::substrait::proto::{Plan, Rel, plan_rel};
use prost::Message;

type TableName = Vec<String>;
type TableBindings = HashMap<TableName, LocalFiles>;

/// Produces a serialized Substrait plan whose reads are directly resolvable by Sirius.
pub async fn prepare_query(ctx: &SessionContext, sql: &str) -> Result<Vec<u8>> {
    let frame = ctx.sql(sql).await?;
    let (state, plan) = frame.into_parts();
    let plan = state.optimize(&plan)?;
    let mut plan = *to_substrait_plan(&plan, &state)?;
    let table_names = named_tables(&plan)?;
    let bindings = bindings_from_context(ctx, table_names).await?;
    bind_named_tables(&mut plan, &bindings)?;
    Ok(plan.encode_to_vec())
}

async fn bindings_from_context(
    ctx: &SessionContext,
    table_names: BTreeSet<TableName>,
) -> Result<TableBindings> {
    let mut bindings = HashMap::with_capacity(table_names.len());
    for name in table_names {
        let reference = table_reference(&name)?;
        let provider = ctx.table_provider(reference).await?;
        let listing = provider
            .as_any()
            .downcast_ref::<ListingTable>()
            .ok_or_else(|| {
                DataFusionError::NotImplemented(format!(
                    "Sirius can only bind DataFusion ListingTable inputs; table {} uses a different provider",
                    name.join(".")
                ))
            })?;
        bindings.insert(name, local_files(listing)?);
    }
    Ok(bindings)
}

fn table_reference(name: &[String]) -> Result<TableReference> {
    match name {
        [table] => Ok(TableReference::bare(table.clone())),
        [schema, table] => Ok(TableReference::partial(schema.clone(), table.clone())),
        [catalog, schema, table] => Ok(TableReference::full(
            catalog.clone(),
            schema.clone(),
            table.clone(),
        )),
        _ => Err(DataFusionError::Plan(format!(
            "Substrait named table must have one to three name components, got {:?}",
            name
        ))),
    }
}

fn local_files(table: &ListingTable) -> Result<LocalFiles> {
    let options = table.options();
    if options.file_extension != ".parquet" {
        return Err(DataFusionError::NotImplemented(format!(
            "Sirius only supports Parquet ListingTable inputs, got extension {:?}",
            options.file_extension
        )));
    }
    if !options.table_partition_cols.is_empty() {
        return Err(DataFusionError::NotImplemented(
            "Sirius does not yet support DataFusion listing-table partition columns".to_string(),
        ));
    }
    if table.table_paths().is_empty() {
        return Err(DataFusionError::Plan(
            "DataFusion ListingTable has no paths".to_string(),
        ));
    }

    let items = table
        .table_paths()
        .iter()
        .map(|url| local_file_item(url, &options.file_extension))
        .collect::<Result<Vec<_>>>()?;
    Ok(LocalFiles {
        items,
        ..Default::default()
    })
}

fn local_file_item(url: &ListingTableUrl, extension: &str) -> Result<FileOrFiles> {
    if url.get_url().scheme() != "file" {
        return Err(DataFusionError::NotImplemented(format!(
            "Sirius currently supports local file ListingTable URLs, got {url}"
        )));
    }
    if url.get_glob().is_some() {
        return Err(DataFusionError::NotImplemented(format!(
            "Sirius does not yet support DataFusion ListingTable globs: {url}"
        )));
    }
    let path = url.get_url().to_file_path().map_err(|()| {
        DataFusionError::Plan(format!(
            "could not convert ListingTable URL to a path: {url}"
        ))
    })?;
    let path = if url.is_folder() {
        directory_glob(&path, extension)
    } else {
        path
    };
    let path = path.to_str().ok_or_else(|| {
        DataFusionError::Plan(format!(
            "Parquet path is not valid UTF-8: {}",
            path.display()
        ))
    })?;

    Ok(FileOrFiles {
        path_type: Some(PathType::UriPath(path.to_string())),
        file_format: Some(FileFormat::Parquet(ParquetReadOptions {})),
        ..Default::default()
    })
}

fn directory_glob(path: &Path, extension: &str) -> PathBuf {
    path.join(format!("*{extension}"))
}

fn named_tables(plan: &Plan) -> Result<BTreeSet<TableName>> {
    let mut tables = BTreeSet::new();
    for relation in &plan.relations {
        match relation.rel_type.as_ref() {
            Some(plan_rel::RelType::Rel(rel)) => collect_named_tables(rel, &mut tables)?,
            Some(plan_rel::RelType::Root(root)) => {
                visit_input(root.input.as_ref().map(|input| input as &Rel), |rel| {
                    collect_named_tables(rel, &mut tables)
                })?
            }
            None => {
                return Err(DataFusionError::Plan(
                    "Substrait PlanRel is missing its relation type".to_string(),
                ));
            }
        }
    }
    Ok(tables)
}

fn collect_named_tables(rel: &Rel, tables: &mut BTreeSet<TableName>) -> Result<()> {
    walk_rel(rel, &mut |read| {
        if let Some(ReadType::NamedTable(table)) = &read.read_type {
            tables.insert(table.names.clone());
        }
        Ok(())
    })
}

fn bind_named_tables(plan: &mut Plan, bindings: &TableBindings) -> Result<usize> {
    let mut bound = 0;
    for relation in &mut plan.relations {
        match relation.rel_type.as_mut() {
            Some(plan_rel::RelType::Rel(rel)) => {
                bind_rel(rel, bindings, &mut bound)?;
            }
            Some(plan_rel::RelType::Root(root)) => visit_input_mut(root.input.as_mut(), |rel| {
                bind_rel(rel, bindings, &mut bound)
            })?,
            None => {
                return Err(DataFusionError::Plan(
                    "Substrait PlanRel is missing its relation type".to_string(),
                ));
            }
        }
    }
    Ok(bound)
}

fn bind_rel(rel: &mut Rel, bindings: &TableBindings, bound: &mut usize) -> Result<()> {
    walk_rel_mut(rel, &mut |read| {
        if let Some(ReadType::NamedTable(table)) = &read.read_type {
            let files = bindings.get(&table.names).ok_or_else(|| {
                DataFusionError::Plan(format!(
                    "no Parquet binding for Substrait table {}",
                    table.names.join(".")
                ))
            })?;
            read.read_type = Some(ReadType::LocalFiles(files.clone()));
            *bound += 1;
        }
        Ok(())
    })
}

fn walk_rel(
    rel: &Rel,
    visit_read: &mut impl FnMut(&datafusion_substrait::substrait::proto::ReadRel) -> Result<()>,
) -> Result<()> {
    let rel_type = rel
        .rel_type
        .as_ref()
        .ok_or_else(|| DataFusionError::Plan("Substrait Rel is missing its type".to_string()))?;
    match rel_type {
        RelType::Read(read) => visit_read(read),
        RelType::Filter(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Fetch(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Aggregate(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Sort(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Project(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::ExtensionSingle(rel) => {
            visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read))
        }
        RelType::Window(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Exchange(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Expand(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Write(rel) => visit_input(rel.input.as_deref(), |v| walk_rel(v, visit_read)),
        RelType::Ddl(rel) => {
            visit_input(rel.view_definition.as_deref(), |v| walk_rel(v, visit_read))
        }
        RelType::Join(rel) => visit_pair(rel.left.as_deref(), rel.right.as_deref(), |v| {
            walk_rel(v, visit_read)
        }),
        RelType::Cross(rel) => visit_pair(rel.left.as_deref(), rel.right.as_deref(), |v| {
            walk_rel(v, visit_read)
        }),
        RelType::HashJoin(rel) => visit_pair(rel.left.as_deref(), rel.right.as_deref(), |v| {
            walk_rel(v, visit_read)
        }),
        RelType::MergeJoin(rel) => visit_pair(rel.left.as_deref(), rel.right.as_deref(), |v| {
            walk_rel(v, visit_read)
        }),
        RelType::NestedLoopJoin(rel) => {
            visit_pair(rel.left.as_deref(), rel.right.as_deref(), |v| {
                walk_rel(v, visit_read)
            })
        }
        RelType::Set(rel) => visit_inputs(&rel.inputs, |v| walk_rel(v, visit_read)),
        RelType::ExtensionMulti(rel) => visit_inputs(&rel.inputs, |v| walk_rel(v, visit_read)),
        RelType::ExtensionLeaf(_) | RelType::Reference(_) | RelType::Update(_) => Ok(()),
    }
}

fn walk_rel_mut(
    rel: &mut Rel,
    visit_read: &mut impl FnMut(&mut datafusion_substrait::substrait::proto::ReadRel) -> Result<()>,
) -> Result<()> {
    let rel_type = rel
        .rel_type
        .as_mut()
        .ok_or_else(|| DataFusionError::Plan("Substrait Rel is missing its type".to_string()))?;
    match rel_type {
        RelType::Read(read) => visit_read(read),
        RelType::Filter(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Fetch(rel) => visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read)),
        RelType::Aggregate(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Sort(rel) => visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read)),
        RelType::Project(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::ExtensionSingle(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Window(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Exchange(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Expand(rel) => {
            visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read))
        }
        RelType::Write(rel) => visit_input_mut(rel.input.as_mut(), |v| walk_rel_mut(v, visit_read)),
        RelType::Ddl(rel) => visit_input_mut(rel.view_definition.as_mut(), |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::Join(rel) => visit_pair_mut(&mut rel.left, &mut rel.right, |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::Cross(rel) => visit_pair_mut(&mut rel.left, &mut rel.right, |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::HashJoin(rel) => visit_pair_mut(&mut rel.left, &mut rel.right, |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::MergeJoin(rel) => visit_pair_mut(&mut rel.left, &mut rel.right, |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::NestedLoopJoin(rel) => visit_pair_mut(&mut rel.left, &mut rel.right, |v| {
            walk_rel_mut(v, visit_read)
        }),
        RelType::Set(rel) => visit_inputs_mut(&mut rel.inputs, |v| walk_rel_mut(v, visit_read)),
        RelType::ExtensionMulti(rel) => {
            visit_inputs_mut(&mut rel.inputs, |v| walk_rel_mut(v, visit_read))
        }
        RelType::ExtensionLeaf(_) | RelType::Reference(_) | RelType::Update(_) => Ok(()),
    }
}

fn visit_input<T>(input: Option<&T>, mut visit: impl FnMut(&T) -> Result<()>) -> Result<()> {
    input.map(&mut visit).transpose().map(|_| ())
}

fn visit_input_mut<T>(
    input: Option<&mut T>,
    mut visit: impl FnMut(&mut T) -> Result<()>,
) -> Result<()> {
    input.map(&mut visit).transpose().map(|_| ())
}

fn visit_pair<T>(
    left: Option<&T>,
    right: Option<&T>,
    mut visit: impl FnMut(&T) -> Result<()>,
) -> Result<()> {
    visit_input(left, &mut visit)?;
    visit_input(right, visit)
}

fn visit_pair_mut<T>(
    left: &mut Option<Box<T>>,
    right: &mut Option<Box<T>>,
    mut visit: impl FnMut(&mut T) -> Result<()>,
) -> Result<()> {
    visit_input_mut(left.as_deref_mut(), &mut visit)?;
    visit_input_mut(right.as_deref_mut(), visit)
}

fn visit_inputs<T>(inputs: &[T], mut visit: impl FnMut(&T) -> Result<()>) -> Result<()> {
    inputs.iter().try_for_each(&mut visit)
}

fn visit_inputs_mut<T>(
    inputs: &mut [T],
    mut visit: impl FnMut(&mut T) -> Result<()>,
) -> Result<()> {
    inputs.iter_mut().try_for_each(&mut visit)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::{ParquetReadOptions, SessionContext};
    use parquet::arrow::ArrowWriter;

    use super::{ReadType, named_tables, prepare_query};
    use datafusion_substrait::substrait::proto::Plan;
    use prost::Message;

    fn parquet_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part-0.parquet");
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let values: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![values]).unwrap();
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(&path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn binds_all_join_inputs_to_local_parquet() {
        let (dir, _) = parquet_fixture();
        let ctx = SessionContext::new();
        for table in ["left_rows", "right_rows"] {
            ctx.register_parquet(
                table,
                dir.path().to_str().unwrap(),
                ParquetReadOptions::default(),
            )
            .await
            .unwrap();
        }

        let bytes = prepare_query(
            &ctx,
            "SELECT l.id FROM left_rows l JOIN right_rows r ON l.id = r.id",
        )
        .await
        .unwrap();
        let plan = Plan::decode(bytes.as_slice()).unwrap();
        assert!(named_tables(&plan).unwrap().is_empty());

        let mut local_reads = 0;
        for relation in &plan.relations {
            let rel = match relation.rel_type.as_ref().unwrap() {
                datafusion_substrait::substrait::proto::plan_rel::RelType::Rel(rel) => rel,
                datafusion_substrait::substrait::proto::plan_rel::RelType::Root(root) => {
                    root.input.as_ref().unwrap()
                }
            };
            super::walk_rel(rel, &mut |read| {
                if let Some(ReadType::LocalFiles(files)) = &read.read_type {
                    local_reads += 1;
                    assert_eq!(files.items.len(), 1);
                    let path = match files.items[0].path_type.as_ref().unwrap() {
                        super::PathType::UriPath(path) => path,
                        other => panic!("unexpected path type: {other:?}"),
                    };
                    assert!(path.ends_with("*.parquet"));
                }
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(local_reads, 2);
    }

    #[tokio::test]
    async fn rejects_non_listing_table_inputs() {
        let ctx = SessionContext::new();
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let values: ArrayRef = Arc::new(Int64Array::from(vec![1]));
        ctx.register_batch(
            "memory_rows",
            RecordBatch::try_new(schema, vec![values]).unwrap(),
        )
        .unwrap();

        let error = prepare_query(&ctx, "SELECT * FROM memory_rows")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ListingTable"));
    }

    #[tokio::test]
    async fn plans_tpch_datetime_and_unicode_expressions() {
        let ctx = SessionContext::new();
        ctx.sql(
            "SELECT EXTRACT(YEAR FROM DATE '1995-01-01'), \
             SUBSTRING('abcdef' FROM 1 FOR 2)",
        )
        .await
        .unwrap();
    }
}
