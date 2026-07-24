//! Safe, idiomatic Rust bindings for [Sirius](https://github.com/sirius-db/sirius),
//! the GPU-native SQL engine.
//!
//! This crate wraps the low-level [`sirius-sys`] cxx bindings in safe Rust types
//! — the entry point for driving Sirius from Rust.
//!
//! In addition to context construction and Substrait execution, it provides
//! ownership-safe [`DataRepository`] and [`DataBatch`] wrappers for the native
//! cuCascade data boundary. The third-party C++ types never cross the public FFI.

use std::path::Path;

use arrow_array::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::SchemaRef;
use cxx::{Exception, UniquePtr, let_cxx_string};

/// An initialized Sirius engine context.
///
/// Constructing one brings up the engine (GPU resources included); dropping it
/// tears the engine down. The `cxx::UniquePtr` owns the C++ object, so lifetime
/// is pure RAII — there is no uninitialized or manually-freed state.
///
/// Bring-up is fallible: GPU initialization or parsing a config file can fail,
/// surfaced here as a [`cxx::Exception`].
///
/// The engine keeps process-global GPU state, so it currently supports a single
/// live context per process; constructing or holding more than one concurrently
/// is not yet supported (enforcement is a follow-up).
pub struct SiriusContext {
    // RAII handle owning the C++ engine context for its lifetime.
    inner: UniquePtr<sirius_sys::Context>,
}

/// A shared native data repository associated with a [`SiriusContext`].
///
/// The C++ handle owns a shared reference to the underlying repository, allowing
/// a future native streaming source or sink to use the same repository. The
/// Rust lifetime prevents the repository—and any batch extracted from it—from
/// outliving the context that owns the batch memory spaces.
///
/// Batches transfer into the repository by value and transfer back out through
/// [`pop_next`](Self::pop_next). This keeps ownership explicit at the Rust API
/// even though the native implementation uses shared pointers internally.
pub struct DataRepository<'context> {
    inner: UniquePtr<sirius_sys::DataRepository>,
    context: &'context SiriusContext,
}

/// An owned handle to a native cuCascade data batch.
///
/// Sirius's C++ adapter holds the native batch by shared ownership while Rust
/// sees only this opaque Sirius-defined type. A batch can be inspected for its
/// immutable ID or transferred into a [`DataRepository`].
pub struct DataBatch<'context> {
    inner: UniquePtr<sirius_sys::DataBatch>,
    context: &'context SiriusContext,
}

/// Fully materialized output of one Substrait execution.
pub struct SubstraitResult {
    /// Arrow schema reported by the result stream, also available for empty results.
    pub schema: SchemaRef,
    /// Eagerly collected output batches.
    pub batches: Vec<RecordBatch>,
}

impl SiriusContext {
    /// Bring up a new, initialized Sirius engine context configured from
    /// built-in defaults.
    pub fn new() -> Result<Self, Exception> {
        Ok(Self {
            inner: sirius_sys::make_context()?,
        })
    }

    /// Bring up a new, initialized Sirius engine context configured from the
    /// YAML config file at `path`.
    pub fn from_config_file(path: &Path) -> Result<Self, Exception> {
        // cxx passes the path to the C++ `const std::string&` parameter; build
        // one from the (lossy) UTF-8 form of the platform path.
        let_cxx_string!(config_path = path.to_string_lossy().as_ref());
        Ok(Self {
            inner: sirius_sys::make_context_from_config(&config_path)?,
        })
    }

    /// Create an empty shared data repository associated with this context.
    ///
    /// The repository starts with partition `0`. Use
    /// [`DataRepository::set_num_partitions`] to grow it before addressing
    /// additional partitions.
    pub fn create_data_repository(&self) -> DataRepository<'_> {
        DataRepository {
            inner: sirius_sys::make_data_repository(),
            context: self,
        }
    }

    /// Execute a serialized Substrait plan on the GPU and return the result rows.
    ///
    /// `plan` is the protobuf-encoded `substrait::Plan`; its reads must be
    /// resolvable by the embedded DuckDB used for lowering (e.g. `local_files`
    /// parquet reads), and every operator must be one the GPU engine supports —
    /// there is no CPU fallback here, so an unsupported plan returns an error.
    ///
    /// The result is collected eagerly into owned [`RecordBatch`]es. DuckDB's
    /// Arrow stream converts each batch using this context's client state, so the
    /// stream is drained while the context is alive; the returned batches own
    /// their buffers and are independent of the context's lifetime.
    pub fn execute_substrait(&mut self, plan: &[u8]) -> Result<Vec<RecordBatch>, SiriusError> {
        Ok(self.execute_substrait_result(plan)?.batches)
    }

    /// Execute a serialized Substrait plan and retain its Arrow schema for empty results.
    pub fn execute_substrait_result(
        &mut self,
        plan: &[u8],
    ) -> Result<SubstraitResult, SiriusError> {
        // The engine writes a self-owning Arrow C Data Interface stream into
        // `stream`; the FFI takes its address as an integer (a `uintptr_t`).
        let mut stream = FFI_ArrowArrayStream::empty();
        let out_stream_addr = std::ptr::addr_of_mut!(stream) as usize;
        let_cxx_string!(plan = plan);
        // SAFETY: `out_stream_addr` is the address of `stream`, a live, writable
        // `FFI_ArrowArrayStream` owned by this stack frame for the call's duration.
        unsafe {
            self.inner
                .pin_mut()
                .execute_substrait(&plan, out_stream_addr)
                .map_err(SiriusError::Engine)?;
        }
        // Drain fully while `self` is alive (conversion dereferences the context).
        let reader = ArrowArrayStreamReader::try_new(stream).map_err(SiriusError::Arrow)?;
        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(SiriusError::Arrow)?;
        Ok(SubstraitResult { schema, batches })
    }
}

impl<'context> DataRepository<'context> {
    /// Transfer `batch` into `partition_idx`.
    ///
    /// The batch is consumed even if the native operation reports an error.
    ///
    /// # Panics
    ///
    /// Panics if the batch belongs to a different [`SiriusContext`].
    pub fn push(
        &mut self,
        batch: DataBatch<'context>,
        partition_idx: usize,
    ) -> Result<(), Exception> {
        assert!(
            std::ptr::eq(self.context, batch.context),
            "cannot push a data batch into a repository from another Sirius context"
        );
        self.inner.pin_mut().push(batch.inner, partition_idx)
    }

    /// Remove the next batch from `partition_idx`, or return `None` when empty.
    pub fn pop_next(
        &mut self,
        partition_idx: usize,
    ) -> Result<Option<DataBatch<'context>>, Exception> {
        let batch = self.inner.pin_mut().pop_next(partition_idx)?;
        if batch.is_null() {
            Ok(None)
        } else {
            Ok(Some(DataBatch {
                inner: batch,
                context: self.context,
            }))
        }
    }

    /// Return the number of batches in `partition_idx`.
    pub fn len(&self, partition_idx: usize) -> Result<usize, Exception> {
        self.inner.size(partition_idx)
    }

    /// Return whether `partition_idx` contains no batches.
    pub fn is_empty(&self, partition_idx: usize) -> Result<bool, Exception> {
        Ok(self.len(partition_idx)? == 0)
    }

    /// Return the number of batches across all partitions.
    pub fn total_len(&self) -> Result<usize, Exception> {
        self.inner.total_size()
    }

    /// Return the current number of partitions.
    pub fn num_partitions(&self) -> Result<usize, Exception> {
        self.inner.num_partitions()
    }

    /// Grow the repository to `new_num_partitions`.
    ///
    /// cuCascade repositories cannot shrink because doing so could discard
    /// batches already in flight.
    pub fn set_num_partitions(&mut self, new_num_partitions: usize) -> Result<(), Exception> {
        self.inner.pin_mut().set_num_partitions(new_num_partitions)
    }
}

impl DataBatch<'_> {
    /// Return the immutable native batch identifier.
    pub fn id(&self) -> u64 {
        self.inner.id()
    }
}

/// Error returned by [`SiriusContext::execute_substrait`].
#[derive(Debug)]
pub enum SiriusError {
    /// The engine failed to translate or execute the plan (a C++ exception).
    Engine(Exception),
    /// The Arrow result stream could not be consumed.
    Arrow(arrow_schema::ArrowError),
}

impl std::fmt::Display for SiriusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(err) => write!(f, "sirius engine error: {err}"),
            Self::Arrow(err) => write!(f, "arrow result error: {err}"),
        }
    }
}

impl std::error::Error for SiriusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Engine(err) => Some(err),
            Self::Arrow(err) => Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use prost::Message;

    use super::SiriusContext;

    /// The engine keeps process-global GPU state, so at most one context may be
    /// live at a time; context-constructing tests hold this for their duration.
    static GPU_CONTEXT_LOCK: Mutex<()> = Mutex::new(());

    /// Proof-of-life: bring up a real Sirius engine context and drop it. This
    /// links the real Sirius library and exercises the full cxx round-trip +
    /// `initialize()`/teardown. Requires a GPU (construction does GPU bring-up).
    #[test]
    fn constructs_and_drops() {
        let _guard = GPU_CONTEXT_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let ctx = SiriusContext::new().expect("bring up default Sirius context");
        let mut repository = ctx.create_data_repository();
        assert_eq!(repository.num_partitions().unwrap(), 1);
        assert!(repository.is_empty(0).unwrap());
        repository.set_num_partitions(2).unwrap();
        assert_eq!(repository.num_partitions().unwrap(), 2);
    }

    /// Encodes a single-file `local_files` parquet read plan with `names` as the
    /// root output names — the shape DuckDB's Substrait reader resolves to
    /// `parquet_scan(<path>)`.
    fn local_files_plan(path: &str, names: Vec<String>) -> Vec<u8> {
        use substrait::proto::read_rel::local_files::FileOrFiles;
        use substrait::proto::read_rel::local_files::file_or_files::{
            FileFormat, ParquetReadOptions, PathType,
        };
        use substrait::proto::read_rel::{LocalFiles, ReadType};
        use substrait::proto::{Plan, PlanRel, ReadRel, Rel, RelRoot, plan_rel, rel};

        let read = Rel {
            rel_type: Some(rel::RelType::Read(Box::new(ReadRel {
                read_type: Some(ReadType::LocalFiles(LocalFiles {
                    items: vec![FileOrFiles {
                        path_type: Some(PathType::UriFile(path.to_string())),
                        file_format: Some(FileFormat::Parquet(ParquetReadOptions {})),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }))),
        };
        Plan {
            relations: vec![PlanRel {
                rel_type: Some(plan_rel::RelType::Root(RelRoot {
                    input: Some(read),
                    names,
                })),
            }],
            ..Default::default()
        }
        .encode_to_vec()
    }

    /// End-to-end: execute a `local_files` parquet plan on the GPU and read the
    /// result rows back over the Arrow C Data Interface. Requires a GPU.
    #[test]
    fn executes_local_files_plan_on_gpu() {
        let _guard = GPU_CONTEXT_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());

        // Point the embedded DuckDB at the locally-built parquet extension (the
        // SIRIUS_BUILD_DIR default mirrors sirius-sys's build.rs) so it can bind
        // `parquet_scan`. Set under the GPU lock so no other context bring-up
        // reads the environment concurrently.
        if std::env::var_os("SIRIUS_DUCKDB_PARQUET_EXTENSION").is_none() {
            let manifest = env!("CARGO_MANIFEST_DIR");
            let build_dir = std::env::var("SIRIUS_BUILD_DIR")
                .unwrap_or_else(|_| format!("{manifest}/../../../build/release"));
            let parquet = format!("{build_dir}/extension/parquet/parquet.duckdb_extension");
            // SAFETY: the GPU lock is held, so no other thread touches the environment here.
            unsafe { std::env::set_var("SIRIUS_DUCKDB_PARQUET_EXTENSION", parquet) };
        }

        // Write a tiny parquet fixture.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.parquet");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let ids: ArrayRef = Arc::new(Int64Array::from(vec![1, 2, 3]));
        let names: ArrayRef = Arc::new(StringArray::from(vec!["a", "b", "c"]));
        let batch = RecordBatch::try_new(schema.clone(), vec![ids, names]).unwrap();
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut writer = ArrowWriter::try_new(file, schema, None).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
        }

        let plan = local_files_plan(
            path.to_str().unwrap(),
            vec!["id".to_string(), "name".to_string()],
        );

        // Execute twice on one context to verify standalone query state is reset,
        // then drop it before inspecting the returned owned results.
        let results = {
            let mut ctx = SiriusContext::new().expect("bring up sirius context");
            vec![
                ctx.execute_substrait_result(&plan)
                    .expect("execute first substrait plan"),
                ctx.execute_substrait_result(&plan)
                    .expect("execute second substrait plan"),
            ]
        };

        for result in results {
            assert_eq!(result.schema.fields().len(), 2);
            assert_eq!(result.schema.field(0).name(), "id");
            assert_eq!(result.schema.field(1).name(), "name");
            let total_rows: usize = result.batches.iter().map(RecordBatch::num_rows).sum();
            assert_eq!(total_rows, 3, "expected 3 rows from the parquet fixture");
        }
    }

    /// A missing config file is rejected before any GPU work (`load_from_file`
    /// throws first), so this exercises the fallible config path and the cxx
    /// exception round-trip without needing a GPU.
    #[test]
    fn missing_config_file_is_an_error() {
        let result = SiriusContext::from_config_file(Path::new(
            "/nonexistent/sirius-config-does-not-exist.yaml",
        ));
        assert!(result.is_err(), "missing config file should fail bring-up");
    }
}
