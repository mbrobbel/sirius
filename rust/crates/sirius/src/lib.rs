//! Safe Rust bindings to the public Sirius C ABI.
//!
//! Build immutable configurations with [`ContextConfigBuilder`] and construct an
//! engine with [`Context`]. These bindings require no C++ compilation.
//! Existing execution and object-store bindings live in the `sirius-ffi` crate.

mod config;
mod context;
mod diagnostic;
pub use config::{ConfigError, ContextConfig, ContextConfigBuilder};
pub use context::{Context, ContextError};
