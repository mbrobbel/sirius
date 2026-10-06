use crate::standard::Column;
use duckdb::{
    Connection,
    arrow::{
        array::{Array, AsArray},
        datatypes::{DataType, Decimal128Type, Float32Type, Float64Type},
        record_batch::RecordBatch,
        util::display::array_value_to_string,
    },
    core::LogicalTypeId,
};
use sqllogictest::{DB, DBOutput};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("standard SQLLogicTest scripts use one connection")]
    ConnectionAlreadyUsed,
    #[error(transparent)]
    DuckDB(#[from] duckdb::Error),
    #[error(transparent)]
    Arrow(#[from] duckdb::arrow::error::ArrowError),
    #[error(transparent)]
    Real(#[from] std::num::ParseFloatError),
}

pub struct Database {
    connection: Connection,
    sirius: bool,
}

impl Database {
    pub fn new(connection: Connection, sirius: bool) -> Self {
        Self { connection, sirius }
    }
}

impl DB for Database {
    type Error = Error;
    type ColumnType = Column;

    fn engine_name(&self) -> &str {
        if self.sirius { "sirius" } else { "duckdb" }
    }

    fn run(&mut self, sql: &str) -> Result<DBOutput<Column>, Error> {
        let mut prepared = self.connection.prepare(sql)?;
        prepared.execute([])?;
        let logical_types: Vec<_> = (0..prepared.column_count())
            .map(|index| prepared.column_logical_type(index).id())
            .collect();
        let types: Vec<_> = prepared
            .schema()
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| match field.data_type() {
                ty if ty.is_integer() => Column::Integer,
                DataType::Decimal128(_, 0)
                    if matches!(
                        logical_types[index],
                        LogicalTypeId::Hugeint | LogicalTypeId::UHugeint
                    ) =>
                {
                    Column::Integer
                }
                ty if ty.is_floating() || ty.is_decimal() => Column::Real,
                _ => Column::Text,
            })
            .collect();
        let mut rows = Vec::new();
        while let Some(array) = prepared.step()? {
            let batch = RecordBatch::from(&array);
            for row in 0..batch.num_rows() {
                let mut values = Vec::new();
                for (index, (column, ty)) in batch.columns().iter().zip(&types).enumerate() {
                    let value = if column.is_null(row) {
                        "NULL".to_owned()
                    } else {
                        let value = array_value_to_string(column, row)?;
                        match ty {
                            Column::Real => {
                                let number = match column.data_type() {
                                    DataType::Float32 => {
                                        f64::from(column.as_primitive::<Float32Type>().value(row))
                                    }
                                    DataType::Float64 => {
                                        column.as_primitive::<Float64Type>().value(row)
                                    }
                                    _ => value.parse::<f64>()?,
                                };
                                format!("{number:.3}").to_ascii_lowercase()
                            }
                            Column::Text if value.is_empty() => "(empty)".to_owned(),
                            Column::Text => value
                                .bytes()
                                .map(|byte| {
                                    if (b' '..=b'~').contains(&byte) {
                                        byte as char
                                    } else {
                                        '@'
                                    }
                                })
                                .collect(),
                            Column::Integer if logical_types[index] == LogicalTypeId::UHugeint => {
                                (column.as_primitive::<Decimal128Type>().value(row) as u128)
                                    .to_string()
                            }
                            Column::Integer => value,
                        }
                    };
                    values.push(value);
                }
                rows.push(values);
            }
        }
        Ok(DBOutput::Rows { types, rows })
    }
}
