//! Safe, idiomatic Rust bindings for [Sirius](https://github.com/sirius-db/sirius),
//! the GPU-native SQL engine.
//!
//! This crate wraps the low-level [`sirius-sys`][sirius_sys] cxx bindings in safe Rust types
//! — the entry point for driving Sirius from Rust.
//!
//! Execute Substrait plans directly, or bootstrap exchange peers and run a
//! [`SiriusFragment`] as part of a distributed query.

use std::marker::PhantomData;
use std::path::Path;
use std::time::Duration;

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

/// Fully materialized output of one Substrait execution.
pub struct SubstraitResult {
    /// Arrow schema reported by the result stream, also available for empty results.
    pub schema: SchemaRef,
    /// Eagerly collected output batches.
    pub batches: Vec<RecordBatch>,
}

/// A built fragment that exclusively borrows its context through execution.
pub struct SiriusFragment<'context> {
    inner: UniquePtr<sirius_sys::Fragment>,
    _context: PhantomData<&'context mut SiriusContext>,
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

    /// Initialize the NIXL exchange transport before registering peers or building a fragment.
    ///
    /// Each process needs a unique `agent_name`. `staging_bytes` bounds the
    /// transport's staging buffer; large batches are transferred in chunks.
    pub fn enable_exchange(
        &mut self,
        agent_name: &str,
        staging_bytes: usize,
        timeout: Duration,
    ) -> Result<(), SiriusError> {
        let timeout_ms = u64::try_from(timeout.as_millis())
            .map_err(|_| SiriusError::InvalidInput("exchange timeout exceeds u64 milliseconds"))?;
        let_cxx_string!(agent_name = agent_name);
        self.inner
            .pin_mut()
            .enable_exchange(&agent_name, staging_bytes, timeout_ms)
            .map_err(SiriusError::Engine)
    }

    /// Return this agent's opaque bootstrap metadata for an external coordinator.
    pub fn exchange_metadata(&self) -> Result<Vec<u8>, Exception> {
        let metadata = self.inner.exchange_metadata()?;
        Ok(metadata.as_bytes().to_vec())
    }

    /// Import a peer's opaque bootstrap metadata and return its routing name.
    pub fn add_exchange_peer(&mut self, metadata: &[u8]) -> Result<String, Exception> {
        let_cxx_string!(metadata = metadata);
        let name = self.inner.pin_mut().add_exchange_peer(&metadata)?;
        Ok(name.to_string_lossy().into_owned())
    }

    /// Build an exchange fragment from a serialized Substrait plan.
    ///
    /// The plan declares its exchange inputs and outputs. The returned fragment
    /// borrows this context until dropped, preventing overlapping query lifecycles.
    pub fn fragment(&mut self, plan: &[u8]) -> Result<SiriusFragment<'_>, Exception> {
        // SAFETY: the lifetime marker keeps the context exclusively borrowed
        // until the owned fragment is dropped.
        let mut inner = unsafe { sirius_sys::make_fragment(self.inner.pin_mut())? };
        let_cxx_string!(plan = plan);
        inner.pin_mut().build(&plan)?;
        Ok(SiriusFragment {
            inner,
            _context: PhantomData,
        })
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
        collect_arrow_stream(stream)
    }
}

impl SiriusFragment<'_> {
    /// Execute until the fragment's pipelines and exchange transfers finish.
    pub fn run(&mut self) -> Result<(), Exception> {
        self.inner.pin_mut().run()
    }

    /// Collect a completed result fragment's Arrow rows while its context is alive.
    ///
    /// Returns an error before `run()`, for an intermediate fragment, or after
    /// the result has already been collected.
    pub fn result(&mut self) -> Result<SubstraitResult, SiriusError> {
        let mut stream = FFI_ArrowArrayStream::empty();
        // SAFETY: `stream` is writable and remains alive throughout the call.
        // Collection below releases it while the fragment still borrows its context.
        unsafe {
            self.inner
                .pin_mut()
                .result_to_arrow(std::ptr::addr_of_mut!(stream) as usize)
                .map_err(SiriusError::Engine)?;
        }
        collect_arrow_stream(stream)
    }
}

fn collect_arrow_stream(stream: FFI_ArrowArrayStream) -> Result<SubstraitResult, SiriusError> {
    let reader = ArrowArrayStreamReader::try_new(stream).map_err(SiriusError::Arrow)?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(SiriusError::Arrow)?;
    Ok(SubstraitResult { schema, batches })
}

/// Error returned while configuring or executing a Sirius query.
#[derive(Debug)]
pub enum SiriusError {
    /// A Rust argument cannot be represented by the engine API.
    InvalidInput(&'static str),
    /// The engine failed to translate or execute the plan (a C++ exception).
    Engine(Exception),
    /// The Arrow result stream could not be consumed.
    Arrow(arrow_schema::ArrowError),
}

impl std::fmt::Display for SiriusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput(err) => write!(f, "invalid sirius input: {err}"),
            Self::Engine(err) => write!(f, "sirius engine error: {err}"),
            Self::Arrow(err) => write!(f, "arrow result error: {err}"),
        }
    }
}

impl std::error::Error for SiriusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidInput(_) => None,
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
        let _ctx = SiriusContext::new().expect("bring up default Sirius context");
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
