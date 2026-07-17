//! Sirius execution for DataFusion logical plans.
//!
//! The public boundary uses DataFusion's Arrow types so this crate can be added directly to the
//! `libcudf-datafusion-benchmarks` runner. The Sirius dependency selects the same Arrow version,
//! making its result batches directly consumable by DataFusion.

pub mod compare;
pub mod plan;

#[cfg(feature = "sirius-engine")]
mod engine;

#[cfg(feature = "sirius-engine")]
use std::path::PathBuf;
#[cfg(feature = "sirius-engine")]
use std::sync::Arc;

#[cfg(feature = "sirius-engine")]
use arrow_array::RecordBatch;
#[cfg(feature = "sirius-engine")]
use datafusion::error::{DataFusionError, Result};
#[cfg(feature = "sirius-engine")]
use datafusion::prelude::SessionContext;

#[cfg(feature = "sirius-engine")]
use crate::engine::SiriusExecutor;

/// Sirius query execution that can be called from a DataFusion benchmark runner.
#[cfg(feature = "sirius-engine")]
#[derive(Clone, Debug)]
pub struct SiriusDataFusion {
    executor: Arc<SiriusExecutor>,
}

#[cfg(feature = "sirius-engine")]
impl SiriusDataFusion {
    /// Starts Sirius with its built-in configuration.
    pub fn new() -> Result<Self> {
        Self::start(None)
    }

    /// Starts Sirius using a YAML configuration file.
    pub fn from_config_file(path: impl Into<PathBuf>) -> Result<Self> {
        Self::start(Some(path.into()))
    }

    fn start(config: Option<PathBuf>) -> Result<Self> {
        let executor = SiriusExecutor::start(config).map_err(DataFusionError::Execution)?;
        Ok(Self {
            executor: Arc::new(executor),
        })
    }

    /// Plans `sql` with `ctx`, binds its registered Parquet tables, and executes it in Sirius.
    pub async fn execute_query(&self, ctx: &SessionContext, sql: &str) -> Result<Vec<RecordBatch>> {
        let plan = plan::prepare_query(ctx, sql).await?;
        let executor = Arc::clone(&self.executor);
        tokio::task::spawn_blocking(move || executor.execute(plan))
            .await
            .map_err(|err| {
                DataFusionError::Execution(format!("Sirius execution task failed: {err}"))
            })?
            .map_err(DataFusionError::Execution)
    }
}

#[cfg(all(test, feature = "sirius-engine"))]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Array, ArrayRef, Int64Array, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::prelude::{ParquetReadOptions, SessionContext};
    use parquet::arrow::ArrowWriter;

    use super::SiriusDataFusion;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires a built Sirius engine and a CUDA GPU"]
    async fn executes_datafusion_query_with_sirius() {
        if std::env::var_os("SIRIUS_DUCKDB_PARQUET_EXTENSION").is_none() {
            let build_dir = std::env::var("SIRIUS_BUILD_DIR")
                .unwrap_or_else(|_| format!("{}/../../build/release", env!("CARGO_MANIFEST_DIR")));
            let parquet = format!("{build_dir}/extension/parquet/parquet.duckdb_extension");
            // SAFETY: this is the only context-constructing test in this test process, and the
            // variable is set before the engine thread is started.
            unsafe { std::env::set_var("SIRIUS_DUCKDB_PARQUET_EXTENSION", parquet) };
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("part-0.parquet");
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let values: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3]));
        let batch = RecordBatch::try_new(Arc::clone(&schema), vec![values]).unwrap();
        let mut writer =
            ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema, None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();

        let ctx = SessionContext::new();
        ctx.register_parquet(
            "rows",
            dir.path().to_str().unwrap(),
            ParquetReadOptions::default(),
        )
        .await
        .unwrap();
        let sql = "SELECT id FROM rows WHERE id >= 2 ORDER BY id";
        let expected = ctx.sql(sql).await.unwrap().collect().await.unwrap();

        let sirius = SiriusDataFusion::new().unwrap();
        let actual = sirius.execute_query(&ctx, sql).await.unwrap();
        assert_eq!(int64_values(&actual), int64_values(&expected));
    }

    fn int64_values(batches: &[RecordBatch]) -> Vec<i64> {
        batches
            .iter()
            .flat_map(|batch| {
                batch
                    .column(0)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .unwrap()
                    .iter()
                    .map(Option::unwrap)
            })
            .collect()
    }
}
