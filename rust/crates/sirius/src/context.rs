//! Ownership of an initialized Sirius engine instance.

use crate::diagnostic::Diagnostic;
use sirius_sys::c_api;
use std::ptr::NonNull;

use crate::ContextConfig;

/// A failure while creating an engine context.
#[derive(Debug)]
pub enum ContextError {
    /// The native library reported allocation failure.
    AllocationFailure,
    /// Hardware resolution or engine initialization failed.
    Initialization(String),
    /// An unexpected native status was returned; diagnostics may be empty.
    Unexpected { status: u32, message: String },
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailure => f.write_str("allocation failed"),
            Self::Initialization(message) => f.write_str(message),
            Self::Unexpected { status, message } => write!(f, "Sirius status {status}: {message}"),
        }
    }
}

impl std::error::Error for ContextError {}

/// Own an initialized Sirius engine. Dropping it releases its resources.
///
/// Only one active engine context per process is supported, including contexts
/// created through other Sirius integrations. This restriction is not enforced;
/// callers must ensure context lifetimes do not overlap. Constructing another
/// context may succeed, but shared runtime resources can interfere with each other.
/// Destruction does not reset all process-wide settings: changing
/// `sirius.executor.downgrade.copy_chunk_bytes` after a successful creation is unsupported.
/// CUDA/NVTX initialization persists even after failed creation; NVTX injection settings
/// must remain unchanged for the process lifetime.
/// An unrecoverable failure to stop workers or destroy resources terminates the process.
/// Forking with an active context is unsupported.
///
/// This type owns a native context through the C ABI and supports blocking
/// materialized execution of plans constructed through the public builder.
/// It is neither `Send` nor `Sync`.
pub struct Context {
    // Own the native engine until Rust drops this handle.
    inner: NonNull<c_api::SiriusContext>,
}

impl Context {
    /// Initialize the engine from a validated configuration.
    ///
    /// The configuration need not outlive the context. Hardware discovery and
    /// resource allocation happen here, not when loading the configuration.
    ///
    /// ```no_run
    /// # use sirius::{Context, ContextConfigBuilder};
    /// # fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = ContextConfigBuilder::from_yaml("sirius.yaml")?.build()?;
    /// let context = Context::new(&config)?;
    /// drop(config);
    /// drop(context); // Release the engine.
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: &ContextConfig) -> Result<Self, ContextError> {
        let mut value = std::ptr::null_mut();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: config remains borrowed for the call; output slots are writable and empty.
        // Callers must follow the documented single-context engine restriction.
        let status = unsafe {
            c_api::sirius_context_create(config.inner.as_ptr(), &mut value, &mut diagnostic.0)
        };
        if status != c_api::SIRIUS_SUCCESS {
            return Err(match status {
                c_api::SIRIUS_ALLOCATION_FAILURE => ContextError::AllocationFailure,
                c_api::SIRIUS_CONTEXT_INITIALIZATION => {
                    ContextError::Initialization(diagnostic.message())
                }
                _ => ContextError::Unexpected {
                    status,
                    message: diagnostic.message(),
                },
            });
        }
        Ok(Self {
            inner: NonNull::new(value).expect("successful context creation"),
        })
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: this is the unique owner, and Rust borrows cannot outlive it.
        unsafe { c_api::sirius_context_destroy(self.inner.as_ptr()) }
    }
}

impl Context {
    /// Prepare and execute a constructed plan, returning materialized CPU Arrow batches.
    ///
    /// Blocks until execution and output collection complete. Requires exclusive access
    /// to the context; it remains neither Send nor Sync. The plan is borrowed and can
    /// be reused after success or failure. No DuckDB CPU query fallback is performed.
    ///
    /// ```no_run
    /// # fn run(context: &mut sirius::Context, plan: &sirius::Plan) -> Result<(), sirius::PlanError> {
    /// let result = context.execute_plan(plan)?;
    /// for batch in result.batches { println!("{} rows", batch.num_rows()); }
    /// # Ok(())
    /// # }
    /// ```
    pub fn execute_plan(
        &mut self,
        plan: &crate::Plan,
    ) -> Result<crate::PlanResult, crate::PlanError> {
        use arrow_array::RecordBatchReader;
        use arrow_array::ffi_stream::{ArrowArrayStreamReader, FFI_ArrowArrayStream};
        let mut stream = FFI_ArrowArrayStream::empty();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: both handles remain borrowed, self is exclusive, and stream is an
        // empty writable Arrow C stream. Native execution transfers its ownership.
        let status = unsafe {
            sirius_sys::plan::sirius_context_execute_plan(
                self.inner.as_ptr(),
                plan.inner.as_ptr(),
                (&mut stream as *mut FFI_ArrowArrayStream).cast(),
                &mut diagnostic.0,
            )
        };
        crate::plan::check(status, &diagnostic)?;
        let arrow_error = |error: arrow_schema::ArrowError| crate::PlanError {
            status: sirius_sys::plan::SIRIUS_EXECUTION_FAILURE,
            message: error.to_string(),
        };
        let reader = ArrowArrayStreamReader::try_new(stream).map_err(arrow_error)?;
        let schema = reader.schema();
        let batches = reader.collect::<Result<Vec<_>, _>>().map_err(arrow_error)?;
        Ok(crate::PlanResult { schema, batches })
    }
}
