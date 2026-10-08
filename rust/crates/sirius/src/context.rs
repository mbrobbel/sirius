//! Ownership of an initialized Sirius engine instance.

use cxx::UniquePtr;
use sirius_sys::context::bridge;

use crate::ContextConfig;

/// A failure while creating an engine context.
#[derive(Debug)]
pub enum ContextError {
    /// Another context holds the process runtime, or teardown left it unavailable.
    InUse(String),
    /// Hardware resolution or engine initialization failed.
    Initialization(String),
    /// A native allocation or bridge operation failed.
    Native(cxx::Exception),
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InUse(message) | Self::Initialization(message) => f.write_str(message),
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
/// Only one engine context may be active per process, including contexts created
/// through other Sirius integrations. Creation fails while another context is
/// initializing, active, or shutting down. A failed teardown retains the process
/// reservation until exit. Forking with an active context is unsupported.
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
    /// drop(context); // Release the engine and its process reservation.
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: &ContextConfig) -> Result<Self, ContextError> {
        let result = bridge::context_create(config.inner.as_ref().expect("owned configuration"))?;
        if result.value.is_null() {
            return Err(match result.code {
                bridge::ContextErrorCode::InUse => ContextError::InUse(result.message),
                _ => ContextError::Initialization(result.message),
            });
        }
        Ok(Self {
            _inner: result.value,
        })
    }
}
