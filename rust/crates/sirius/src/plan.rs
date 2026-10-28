//! Immutable context-free plan construction through the C ABI.
//!
//! The first subset supports a single local Parquet scan with an explicit signed
//! 64-bit schema and comparisons against integer literals. Construction does no
//! I/O or GPU work. Unsupported operations return errors instead of falling back.
//!
//! ```no_run
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use sirius::{Comparison, I64Field, Plan, Relation};
//! let scan = Relation::parquet("data.parquet", &[I64Field::new("id", true)])?;
//! let predicate = scan.compare_i64(0, Comparison::Greater, 10)?;
//! let filtered = scan.filter(&predicate)?;
//! let plan = Plan::from_root(&filtered)?;
//! drop(scan); // All dependent descriptions own their metadata.
//! # Ok(())
//! # }
//! ```
use crate::diagnostic::Diagnostic;
use sirius_sys::{c_api, plan as ffi};
use std::path::Path;
use std::ptr::NonNull;

/// A native construction or materialized execution failure.
#[derive(Debug)]
pub struct PlanError {
    /// Fixed-width native status, including unrecognized future statuses.
    pub status: u32,
    /// Best-effort diagnostic; may be empty.
    pub message: String,
}
impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Sirius status {}: {}", self.status, self.message)
    }
}
impl std::error::Error for PlanError {}

pub(crate) fn check(status: u32, diagnostic: &Diagnostic) -> Result<(), PlanError> {
    if status == c_api::SIRIUS_SUCCESS {
        Ok(())
    } else {
        Err(PlanError {
            status,
            message: diagnostic.message(),
        })
    }
}

/// Owned name and nullability for one signed 64-bit column; transferable/shareable.
#[derive(Clone, Debug)]
pub struct I64Field {
    pub name: String,
    pub nullable: bool,
}
impl I64Field {
    /// Describe a field. The scan factory checks names and schema consistency.
    pub fn new(name: impl Into<String>, nullable: bool) -> Self {
        Self {
            name: name.into(),
            nullable,
        }
    }
}
/// SQL comparison; null input yields null, which a filter discards.
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
pub enum Comparison {
    Equal = 0,
    NotEqual = 1,
    Less = 2,
    LessEqual = 3,
    Greater = 4,
    GreaterEqual = 5,
}

/// Immutable relation. Safe to share, construct from, and destroy on different threads.
/// Owns all schema/path data and retains no context or application buffers.
pub struct Relation {
    inner: NonNull<ffi::Relation>,
}
/// Immutable scoped predicate; retains its scope even after the input handle is dropped.
/// Safe for concurrent borrowing and destruction of distinct owning references.
pub struct Expression {
    inner: NonNull<ffi::Expression>,
}
/// Immutable complete plan. Safe to transfer/share; owns all its description data.
/// Releasing construction intermediates does not invalidate it.
pub struct Plan {
    pub(crate) inner: NonNull<ffi::Plan>,
}

// SAFETY: native immutable descriptions retain only owned CPU metadata; reference
// counts are atomic, and destruction invokes no foreign callback or GPU operation.
unsafe impl Send for Relation {}
unsafe impl Sync for Relation {}
unsafe impl Send for Expression {}
unsafe impl Sync for Expression {}
unsafe impl Send for Plan {}
unsafe impl Sync for Plan {}

impl Relation {
    /// Describe one local Parquet file; borrow and copy its explicit i64 schema.
    /// Does not open the file. Multiple calls may share input descriptions.
    pub fn parquet(path: impl AsRef<Path>, fields: &[I64Field]) -> Result<Self, PlanError> {
        use std::os::unix::ffi::OsStrExt;
        let path = path.as_ref().as_os_str().as_bytes();
        let fields: Vec<_> = fields
            .iter()
            .map(|field| ffi::I64Field {
                name: field.name.as_ptr().cast(),
                name_size: field.name.len(),
                nullable: u32::from(field.nullable),
            })
            .collect();
        let mut out = std::ptr::null_mut();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: arguments live through the call; the factory copies their data.
        let status = unsafe {
            ffi::sirius_relation_parquet(
                path.as_ptr().cast(),
                path.len(),
                fields.as_ptr(),
                fields.len(),
                &mut out,
                &mut diagnostic.0,
            )
        };
        check(status, &diagnostic)?;
        Ok(Self {
            inner: NonNull::new(out).expect("successful relation construction"),
        })
    }
    /// Create an immutable comparison scoped to this relation. Column is zero-based.
    pub fn compare_i64(
        &self,
        column: usize,
        comparison: Comparison,
        literal: i64,
    ) -> Result<Expression, PlanError> {
        let mut out = std::ptr::null_mut();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: input remains borrowed and output slots are unique.
        let status = unsafe {
            ffi::sirius_expression_compare_i64(
                self.inner.as_ptr(),
                column,
                comparison as u32,
                literal,
                &mut out,
                &mut diagnostic.0,
            )
        };
        check(status, &diagnostic)?;
        Ok(Expression {
            inner: NonNull::new(out).expect("successful expression construction"),
        })
    }
    /// Return a new immutable relation; fail if the predicate belongs to another scope.
    /// The original relation and predicate remain usable on every outcome.
    pub fn filter(&self, predicate: &Expression) -> Result<Self, PlanError> {
        let mut out = std::ptr::null_mut();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: both handles remain borrowed and output slots are unique.
        let status = unsafe {
            ffi::sirius_relation_filter(
                self.inner.as_ptr(),
                predicate.inner.as_ptr(),
                &mut out,
                &mut diagnostic.0,
            )
        };
        check(status, &diagnostic)?;
        Ok(Self {
            inner: NonNull::new(out).expect("successful filter construction"),
        })
    }
}
impl Plan {
    /// Form a complete single-root plan, retaining all required descriptions.
    pub fn from_root(root: &Relation) -> Result<Self, PlanError> {
        let mut out = std::ptr::null_mut();
        let mut diagnostic = Diagnostic(std::ptr::null_mut());
        // SAFETY: root stays borrowed; output slots are unique.
        let status =
            unsafe { ffi::sirius_plan_from_root(root.inner.as_ptr(), &mut out, &mut diagnostic.0) };
        check(status, &diagnostic)?;
        Ok(Self {
            inner: NonNull::new(out).expect("successful plan construction"),
        })
    }
}
macro_rules! ownership {
    ($ty:ident, $retain:ident, $release:ident) => {
        impl Clone for $ty {
            fn clone(&self) -> Self {
                // SAFETY: acquire one independent reference while self is borrowed.
                unsafe { ffi::$retain(self.inner.as_ptr()) };
                Self { inner: self.inner }
            }
        }
        impl Drop for $ty {
            fn drop(&mut self) {
                // SAFETY: release exactly this wrapper's owning reference.
                unsafe { ffi::$release(self.inner.as_ptr()) };
            }
        }
    };
}
ownership!(Relation, sirius_relation_retain, sirius_relation_release);
ownership!(
    Expression,
    sirius_expression_retain,
    sirius_expression_release
);
ownership!(Plan, sirius_plan_retain, sirius_plan_release);

/// Completed CPU Arrow output. Batches own their buffers independently of the context.
pub struct PlanResult {
    /// Output schema, present even for zero rows.
    pub schema: arrow_schema::SchemaRef,
    /// Materialized batches. This initial API does not provide live streaming.
    pub batches: Vec<arrow_array::RecordBatch>,
}
