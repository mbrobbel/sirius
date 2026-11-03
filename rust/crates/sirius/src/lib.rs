//! Safe Rust bindings to the public Sirius C ABI.
//!
//! Build immutable configurations with [`ContextConfigBuilder`] and construct an
//! engine with [`Context`]. These bindings require no C++ compilation.
//! Construct immutable plans with [`Relation`] and execute them with [`Context::execute_plan`].
//! Existing execution and object-store bindings live in the `sirius-ffi` crate.

mod config;
mod context;
mod diagnostic;
mod plan;
pub use config::{ConfigError, ContextConfig, ContextConfigBuilder};
pub use context::{Context, ContextError};
pub use plan::{Comparison, Expression, I64Field, Plan, PlanError, PlanResult, Relation};
