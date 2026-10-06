use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use substrait::proto::read_rel::local_files::FileOrFiles;
use substrait::proto::read_rel::local_files::file_or_files::{
    FileFormat, ParquetReadOptions, PathType,
};
use substrait::proto::read_rel::{LocalFiles, ReadType};
use substrait::proto::{Plan, PlanRel, ReadRel, Rel, RelRoot, plan_rel, rel};

pub type Row = (Option<i64>, Option<String>);
pub type RowMultiset = BTreeMap<Row, usize>;

pub mod plans;

/// Independent senders share keys and exact duplicate rows; the final sender is empty.
pub fn sender_rows() -> Vec<Vec<Row>> {
    let row = |key, payload: Option<&str>| (key, payload.map(str::to_owned));
    vec![
        vec![
            row(Some(1), Some("first")),
            row(Some(1), Some("duplicate")),
            row(Some(1), Some("duplicate")),
            row(Some(-7), Some("negative")),
            row(None, Some("null key")),
            row(Some(2), None),
            row(Some(i64::MIN), Some("minimum")),
        ],
        vec![
            row(Some(1), Some("duplicate")),
            row(Some(-7), Some("second sender")),
            row(None, Some("null key")),
            row(None, None),
            row(Some(0), Some("")),
            row(Some(2), Some("naïve café")),
            row(Some(i64::MAX), Some("maximum")),
        ],
        vec![],
    ]
}

/// Write the same nullable schema for nonempty and empty sender inputs.
pub fn write_parquet(path: &Path, rows: &[Row]) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("key", DataType::Int64, true),
        Field::new("payload", DataType::Utf8, true),
    ]));
    let keys: ArrayRef = Arc::new(Int64Array::from_iter(rows.iter().map(|row| row.0)));
    let payloads: ArrayRef = Arc::new(StringArray::from_iter(
        rows.iter().map(|row| row.1.as_deref()),
    ));
    let batch = RecordBatch::try_new(schema.clone(), vec![keys, payloads])
        .expect("construct exchange input batch");
    let file = File::create(path).expect("create exchange input parquet file");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("create parquet writer");
    writer.write(&batch).expect("write exchange input batch");
    writer.close().expect("finish exchange input parquet file");
}

pub fn read_parquet(path: &Path) -> Vec<Row> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path).unwrap())
        .unwrap()
        .build()
        .unwrap();
    let batches = reader.collect::<Result<Vec<_>, _>>().unwrap();
    collect_rows(&batches)
}

/// A relation that can be wrapped by an exchange or another Substrait operator.
pub fn local_files_read(path: &Path) -> Rel {
    Rel {
        rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
            read_type: Some(ReadType::LocalFiles(LocalFiles {
                items: vec![FileOrFiles {
                    path_type: Some(PathType::UriFile(
                        path.to_str().expect("UTF-8 fixture path").to_owned(),
                    )),
                    file_format: Some(FileFormat::Parquet(ParquetReadOptions {})),
                    ..Default::default()
                }],
                ..Default::default()
            })),
            ..Default::default()
        }))),
    }
}

pub fn plan(input: Rel) -> Plan {
    Plan {
        relations: vec![PlanRel {
            rel_type: Some(plan_rel::RelType::Root(RelRoot {
                input: Some(input),
                names: vec!["key".to_owned(), "payload".to_owned()],
            })),
        }],
        ..Default::default()
    }
}

/// Preserve nulls and duplicates while ignoring batch boundaries.
pub fn collect_rows(batches: &[RecordBatch]) -> Vec<Row> {
    batches
        .iter()
        .flat_map(|batch| {
            assert_eq!(batch.num_columns(), 2, "expected key and payload columns");
            let keys = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("key column must be Int64");
            let payloads = batch
                .column(1)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("payload column must be Utf8");
            (0..batch.num_rows()).map(move |index| {
                (
                    (!keys.is_null(index)).then(|| keys.value(index)),
                    (!payloads.is_null(index)).then(|| payloads.value(index).to_owned()),
                )
            })
        })
        .collect()
}

pub fn multiset(rows: impl IntoIterator<Item = Row>) -> RowMultiset {
    let mut counts = BTreeMap::new();
    for row in rows {
        *counts.entry(row).or_default() += 1;
    }
    counts
}
