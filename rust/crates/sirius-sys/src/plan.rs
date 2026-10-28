//! Raw immutable plan construction ABI. All pointers follow the public C header contracts.
use crate::c_api::SiriusError;
use std::ffi::c_char;

/// The plan cannot be represented by this builder.
pub const SIRIUS_INVALID_PLAN: u32 = 8;
/// Preparation, execution, or output conversion failed.
pub const SIRIUS_EXECUTION_FAILURE: u32 = 9;

#[repr(C)]
pub struct Relation {
    _private: [u8; 0],
}
#[repr(C)]
pub struct Expression {
    _private: [u8; 0],
}
#[repr(C)]
pub struct Plan {
    _private: [u8; 0],
}
#[repr(C)]
pub struct I64Field {
    pub name: *const c_char,
    pub name_size: usize,
    pub nullable: u32,
}
unsafe extern "C" {
    pub fn sirius_relation_parquet(
        path: *const c_char,
        path_size: usize,
        fields: *const I64Field,
        field_count: usize,
        out: *mut *mut Relation,
        error: *mut *mut SiriusError,
    ) -> u32;
    pub fn sirius_expression_compare_i64(
        input: *const Relation,
        column: usize,
        comparison: u32,
        literal: i64,
        out: *mut *mut Expression,
        error: *mut *mut SiriusError,
    ) -> u32;
    pub fn sirius_relation_filter(
        input: *const Relation,
        predicate: *const Expression,
        out: *mut *mut Relation,
        error: *mut *mut SiriusError,
    ) -> u32;
    pub fn sirius_plan_from_root(
        root: *const Relation,
        out: *mut *mut Plan,
        error: *mut *mut SiriusError,
    ) -> u32;
    pub fn sirius_relation_retain(value: *mut Relation);
    pub fn sirius_relation_release(value: *mut Relation);
    pub fn sirius_expression_retain(value: *mut Expression);
    pub fn sirius_expression_release(value: *mut Expression);
    pub fn sirius_plan_retain(value: *mut Plan);
    pub fn sirius_plan_release(value: *mut Plan);
}
unsafe extern "C" {
    /// out points to an empty ArrowArrayStream, with exclusive access for the call.
    pub fn sirius_context_execute_plan(
        context: *mut crate::c_api::SiriusContext,
        plan: *const Plan,
        out: *mut std::ffi::c_void,
        error: *mut *mut SiriusError,
    ) -> u32;
}
