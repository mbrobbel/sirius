//! Raw declarations matching `include/sirius/c/`.
//!
//! Handles are allocated and released by Sirius. Never free them with Rust's allocator.
//! Output slots must not contain unreleased handles. Diagnostics are optional;
//! always inspect the status even when the diagnostic pointer is null.

use std::ffi::c_char;
use std::marker::{PhantomData, PhantomPinned};

pub const SIRIUS_ABI_VERSION: u32 = 1;
pub const SIRIUS_SUCCESS: u32 = 0;
pub const SIRIUS_CONFIGURATION_IO: u32 = 1;
pub const SIRIUS_MALFORMED_YAML: u32 = 2;
pub const SIRIUS_INVALID_CONFIGURATION: u32 = 3;
pub const SIRIUS_ALLOCATION_FAILURE: u32 = 4;
pub const SIRIUS_INVALID_ARGUMENT: u32 = 5;
pub const SIRIUS_INTERNAL_ERROR: u32 = 6;
pub const SIRIUS_CONTEXT_INITIALIZATION: u32 = 7;

/// Opaque immutable configuration handle.
#[repr(C)]
pub struct SiriusConfig {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
/// Opaque immutable builder handle.
#[repr(C)]
pub struct SiriusConfigBuilder {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
/// Opaque uniquely owned engine handle.
#[repr(C)]
pub struct SiriusContext {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
/// Opaque owned diagnostic.
#[repr(C)]
pub struct SiriusError {
    _private: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

unsafe extern "C" {
    /// Return the C ABI revision implemented by the linked library.
    pub safe fn sirius_abi_version() -> u32;
    /// Borrow diagnostic bytes, terminated by NUL, until the handle is destroyed.
    /// # Safety
    /// `error` must be null or a live diagnostic handle.
    pub fn sirius_error_message(error: *const SiriusError) -> *const c_char;
    /// Return the diagnostic length, excluding its terminator.
    /// # Safety
    /// `error` must be null or a live diagnostic handle.
    pub fn sirius_error_message_size(error: *const SiriusError) -> usize;
    /// Release a diagnostic; null is allowed.
    /// # Safety
    /// A non-null handle must be live and owned by the caller; all borrows must end.
    pub fn sirius_error_destroy(error: *mut SiriusError);
    /// Construct a builder using defaults, without GPU access.
    /// # Safety
    /// `out` must be writable; `error` must be null or writable. Slots must be empty.
    pub fn sirius_config_builder_create(
        out: *mut *mut SiriusConfigBuilder,
        error: *mut *mut SiriusError,
    ) -> u32;
    /// Read and validate a YAML file without GPU access.
    /// # Safety
    /// `path` must address `length` readable bytes. Output slots follow `sirius_config_builder_create`.
    pub fn sirius_config_builder_from_yaml(
        path: *const c_char,
        length: usize,
        out: *mut *mut SiriusConfigBuilder,
        error: *mut *mut SiriusError,
    ) -> u32;
    /// Build an immutable snapshot without accessing hardware.
    /// # Safety
    /// `builder` must remain live; output slots must be writable and empty (`error` may be null).
    pub fn sirius_config_builder_build(
        builder: *const SiriusConfigBuilder,
        out: *mut *mut SiriusConfig,
        error: *mut *mut SiriusError,
    ) -> u32;
    /// Add an owned reference to a builder; null is allowed.
    /// # Safety
    /// A non-null handle must remain live throughout the call.
    pub fn sirius_config_builder_retain(builder: *mut SiriusConfigBuilder);
    /// Release an owned builder reference; null is allowed.
    /// # Safety
    /// The caller must own a reference, and no borrower may outlive the last reference.
    pub fn sirius_config_builder_release(builder: *mut SiriusConfigBuilder);
    /// Add an owned reference to a configuration; null is allowed.
    /// # Safety
    /// A non-null handle must remain live throughout the call.
    pub fn sirius_config_retain(config: *mut SiriusConfig);
    /// Release an owned configuration reference; null is allowed.
    /// # Safety
    /// The caller must own a reference, and no borrower may outlive the last reference.
    pub fn sirius_config_release(config: *mut SiriusConfig);
    /// Resolve hardware and initialize an engine; only one active engine per process is supported.
    /// # Safety
    /// `config` must remain live; output slots must be writable and empty (`error` may be null).
    /// Context lifetimes must not overlap, including engines owned by other integrations.
    pub fn sirius_context_create(
        config: *const SiriusConfig,
        out: *mut *mut SiriusContext,
        error: *mut *mut SiriusError,
    ) -> u32;
    /// Destroy an engine; null is allowed. Unrecoverable teardown failures terminate the process.
    /// # Safety
    /// The caller must uniquely own the live context, and all uses must have ended.
    pub fn sirius_context_destroy(context: *mut SiriusContext);
}
