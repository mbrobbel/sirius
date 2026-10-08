//! Ownership of an initialized Sirius engine instance.

use cxx::UniquePtr;
use sirius_sys::context::bridge;

use crate::ContextConfig;

/// A failure while creating an engine context.
#[derive(Debug)]
pub enum ContextError {
    /// A public C++ factory reported allocation failure without allocating a message.
    AllocationFailure,
    /// Hardware resolution or engine initialization failed.
    Initialization(String),
    /// A C++ exception escaped a bridge operation.
    Native(cxx::Exception),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AllocationFailure => f.write_str("allocation failed"),
            Self::Initialization(message) => f.write_str(message),
            Self::Native(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Native(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cxx::Exception> for ContextError {
    fn from(error: cxx::Exception) -> Self {
        Self::Native(error)
    }
}

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
/// This type owns a public C++ context and exposes construction only. Query
/// execution and client sessions are not exposed by this type yet.
/// It is neither `Send` nor `Sync`.
pub struct Context {
    // Own the native engine until Rust drops this handle.
    _inner: UniquePtr<bridge::Context>,
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
        let result = bridge::context_create(config.inner.as_ref().expect("owned configuration"))?;
        if result.value.is_null() {
            return Err(match result.code {
                bridge::ContextErrorCode::AllocationFailure => ContextError::AllocationFailure,
                _ => ContextError::Initialization(result.message),
            });
        }
        Ok(Self {
            _inner: result.value,
        })
    }
}
