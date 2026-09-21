use anyhow::{Result, ensure};
use arrow::{
    array::{Array, Float32Array, Float64Array, RecordBatch},
    compute::{SortColumn, concat_batches, lexsort_to_indices, take_record_batch},
    datatypes::DataType,
    ipc::{reader::FileReader, writer::FileWriter},
    util::display::array_value_to_string,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, path::Path};

pub fn read(path: &Path) -> Result<RecordBatch> {
    let reader = FileReader::try_new(File::open(path)?, None)?;
    let schema = reader.schema();
    let batches = reader.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(concat_batches(&schema, &batches)?)
}

pub fn write(path: &Path, batch: &RecordBatch) -> Result<()> {
    let mut writer = FileWriter::try_new(File::create(path)?, batch.schema().as_ref())?;
    writer.write(batch)?;
    writer.finish()?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    pub absolute: f64,
    pub relative: f64,
}

impl Tolerance {
    pub fn validate(self) -> Result<()> {
        ensure!(
            self.absolute.is_finite()
                && self.relative.is_finite()
                && self.absolute >= 0.0
                && self.relative >= 0.0,
            "tolerances must be finite and non-negative"
        );
        Ok(())
    }
}

pub type Tolerances = BTreeMap<usize, Tolerance>;

#[derive(Debug)]
pub struct Limitation(pub String);
impl std::fmt::Display for Limitation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Limitation {}

pub fn validate_alignment(
    batch: &RecordBatch,
    ordered: bool,
    tolerances: &Tolerances,
) -> Result<()> {
    let check = (|| -> Result<()> {
        for (&column, &tolerance) in tolerances {
            tolerance.validate()?;
            ensure!(
                column < batch.num_columns(),
                "tolerance column {column} does not exist"
            );
            ensure!(
                matches!(
                    batch.column(column).data_type(),
                    DataType::Float32 | DataType::Float64
                ),
                "tolerance column {column} is not floating point"
            );
        }
        if !ordered && !tolerances.is_empty() {
            let keys: Vec<_> = (0..batch.num_columns())
                .filter(|i| !tolerances.contains_key(i))
                .collect();
            let batch = sorted(batch, &keys)?;
            for row in 1..batch.num_rows() {
                ensure!(
                    keys.iter().any(
                        |&c| batch.column(c).slice(row - 1, 1) != batch.column(c).slice(row, 1)
                    ),
                    "unordered approximate result has duplicate exact keys; use deterministic ordering"
                );
            }
        }
        Ok(())
    })();
    check.map_err(|e| anyhow::Error::new(Limitation(e.to_string())))
}

fn sorted(batch: &RecordBatch, columns: &[usize]) -> Result<RecordBatch> {
    if batch.num_rows() < 2 {
        return Ok(batch.clone());
    }
    ensure!(
        !columns.is_empty(),
        "unordered approximate results need exact key columns or deterministic ordering"
    );
    let sort_columns: Vec<_> = columns
        .iter()
        .map(|&i| SortColumn {
            values: batch.column(i).clone(),
            options: None,
        })
        .collect();
    let indices = lexsort_to_indices(&sort_columns, None)?;
    Ok(take_record_batch(batch, &indices)?)
}

fn float_at(array: &dyn Array, row: usize) -> f64 {
    if let Some(array) = array.as_any().downcast_ref::<Float64Array>() {
        array.value(row)
    } else {
        f64::from(
            array
                .as_any()
                .downcast_ref::<Float32Array>()
                .expect("validated float column")
                .value(row),
        )
    }
}

pub fn compare(
    reference: &RecordBatch,
    actual: &RecordBatch,
    ordered: bool,
    tolerances: &Tolerances,
) -> Result<()> {
    ensure!(
        reference.schema() == actual.schema(),
        "schema mismatch: reference {:?}, actual {:?}",
        reference.schema(),
        actual.schema()
    );
    ensure!(
        reference.num_rows() == actual.num_rows(),
        "row count mismatch: reference {}, actual {}",
        reference.num_rows(),
        actual.num_rows()
    );
    validate_alignment(reference, ordered, tolerances)?;
    validate_alignment(actual, ordered, tolerances)?;
    let keys: Vec<_> = (0..reference.num_columns())
        .filter(|i| !tolerances.contains_key(i))
        .collect();
    let (reference, actual) = if ordered {
        (reference.clone(), actual.clone())
    } else {
        (sorted(reference, &keys)?, sorted(actual, &keys)?)
    };
    for column in 0..reference.num_columns() {
        let (a, b) = (reference.column(column), actual.column(column));
        if let Some(t) = tolerances.get(&column) {
            for row in 0..reference.num_rows() {
                ensure!(
                    a.is_null(row) == b.is_null(row),
                    "NULL mismatch at row {row}, column {column}"
                );
                if a.is_null(row) {
                    continue;
                }
                let (a, b) = (float_at(a.as_ref(), row), float_at(b.as_ref(), row));
                ensure!(
                    (a.is_nan() && b.is_nan())
                        || approx::relative_eq!(
                            a,
                            b,
                            epsilon = t.absolute,
                            max_relative = t.relative
                        ),
                    "float mismatch at row {row}, column {column}: {a} != {b}"
                );
            }
        } else if a != b {
            let row = (0..a.len())
                .find(|&i| a.slice(i, 1) != b.slice(i, 1))
                .unwrap_or(0);
            anyhow::bail!(
                "value mismatch at row {row}, column {column}: {:?} != {:?}",
                array_value_to_string(a, row)?,
                array_value_to_string(b, row)?
            );
        }
    }
    Ok(())
}

pub fn snapshot(batch: &RecordBatch, ordered: bool) -> Result<Vec<String>> {
    let mut rows = Vec::new();
    for row in 0..batch.num_rows() {
        let mut values = Vec::new();
        for column in batch.columns() {
            let value = if column.is_null(row) {
                "NULL".into()
            } else {
                let text = array_value_to_string(column, row)?;
                // Quoting preserves whitespace, embedded separators, and literal "NULL".
                if matches!(
                    column.data_type(),
                    DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View
                ) {
                    serde_json::to_string(&text)?
                } else {
                    text
                }
            };
            values.push(value);
        }
        rows.push(values.join("\t"));
    }
    if !ordered {
        rows.sort();
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, StringArray};
    use std::sync::Arc;
    fn batch(v: Vec<i64>) -> RecordBatch {
        RecordBatch::try_from_iter([("i", Arc::new(Int64Array::from(v)) as arrow::array::ArrayRef)])
            .unwrap()
    }
    #[test]
    fn arrow_preserves_duplicates_and_order() {
        assert!(
            compare(
                &batch(vec![1, 1, 2]),
                &batch(vec![1, 2, 2]),
                false,
                &Tolerances::new()
            )
            .is_err()
        );
        assert!(
            compare(
                &batch(vec![1, 2]),
                &batch(vec![2, 1]),
                false,
                &Tolerances::new()
            )
            .is_ok()
        );
        assert!(
            compare(
                &batch(vec![1, 2]),
                &batch(vec![2, 1]),
                true,
                &Tolerances::new()
            )
            .is_err()
        );
    }
    #[test]
    fn ipc_and_snapshot_preserve_values() -> Result<()> {
        let batch = RecordBatch::try_from_iter([(
            "s",
            Arc::new(StringArray::from(vec![
                None,
                Some("NULL"),
                Some(""),
                Some("a \n\t"),
            ])) as arrow::array::ArrayRef,
        )])?;
        let dir = tempfile::tempdir()?;
        write(&dir.path().join("result.arrow"), &batch)?;
        assert_eq!(batch, read(&dir.path().join("result.arrow"))?);
        assert_eq!(
            snapshot(&batch, true)?,
            vec!["NULL", "\"NULL\"", "\"\"", "\"a \\n\\t\""]
        );
        Ok(())
    }

    #[test]
    fn approximate_columns_require_unambiguous_alignment() -> Result<()> {
        let make = |keys: Vec<i64>, values: Vec<Option<f64>>| {
            RecordBatch::try_from_iter([
                (
                    "key",
                    Arc::new(Int64Array::from(keys)) as arrow::array::ArrayRef,
                ),
                (
                    "value",
                    Arc::new(Float64Array::from(values)) as arrow::array::ArrayRef,
                ),
            ])
            .unwrap()
        };
        let tolerances = [(
            1,
            Tolerance {
                absolute: 1e-6,
                relative: 1e-9,
            },
        )]
        .into();
        let a = make(vec![1, 2, 3], vec![Some(2.0), None, Some(f64::INFINITY)]);
        let b = make(
            vec![3, 1, 2],
            vec![Some(f64::INFINITY), Some(2.0000001), None],
        );
        compare(&a, &b, false, &tolerances)?;
        assert!(compare(&a, &b, true, &tolerances).is_err());
        let duplicate = make(vec![1, 1], vec![Some(2.0), Some(3.0)]);
        assert!(
            compare(&duplicate, &duplicate, false, &tolerances)
                .unwrap_err()
                .is::<Limitation>()
        );
        compare(&duplicate, &duplicate, true, &tolerances)?;
        let large_a = batch(vec![9007199254740992]);
        let large_b = batch(vec![9007199254740993]);
        assert!(compare(&large_a, &large_b, true, &Tolerances::new()).is_err());
        Ok(())
    }
}
