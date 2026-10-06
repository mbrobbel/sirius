//! Low-level `cxx` bindings to the Sirius C++ API.
//!
//! This crate is intentionally thin: it exposes the C++ types and free functions
//! declared in the `#[cxx::bridge]` module below and nothing else. Safe, idiomatic
//! wrappers live in the [`sirius`](https://docs.rs/sirius) crate.
//!
//! The bridge binds Sirius's **public C++ surface** (`include/sirius/ffi.hpp`):
//! an RAII [`Context`] held via [`cxx::UniquePtr`]. Constructing it brings up an
//! initialized engine; dropping the `UniquePtr` tears it down. The header is
//! lightweight, so the bridge compiles without any of Sirius's internal headers
//! (cudf/rmm/duckdb). The bindings link `libsirius` from either the standalone
//! build or the DuckDB extension build — see `build.rs`.
//!
//! The `make_context*` functions are bound as fallible (`Result`): bringing up
//! the engine (or parsing a config file) can throw, and cxx turns a C++ exception
//! into `Err(cxx::Exception)` instead of aborting, so consumers can fail fast.

// The `# Safety` docs on the unsafe bridge fns live on the declarations below;
// cxx's macro expansion hides them from clippy's `missing_safety_doc`, so allow
// it for the generated module.
#[allow(clippy::missing_safety_doc)]
#[cxx::bridge(namespace = "sirius::ffi")]
mod ffi {
    unsafe extern "C++" {
        include!("sirius/ffi.hpp");

        /// RAII handle to an initialized Sirius engine context.
        type Context;

        /// A plan fragment borrowing its engine context.
        type Fragment;

        /// Construct an initialized [`Context`] from built-in defaults, owned by
        /// the returned `UniquePtr`.
        fn make_context() -> Result<UniquePtr<Context>>;

        /// Construct an initialized [`Context`] from the YAML config file at
        /// `config_path`, owned by the returned `UniquePtr`. `config_path` binds
        /// to the C++ `const std::string&` parameter.
        fn make_context_from_config(config_path: &CxxString) -> Result<UniquePtr<Context>>;

        fn enable_exchange(
            self: Pin<&mut Context>,
            agent_name: &CxxString,
            staging_bytes: usize,
            timeout_ms: u64,
        ) -> Result<()>;

        fn exchange_metadata(self: &Context) -> Result<UniquePtr<CxxString>>;

        fn add_exchange_peer(
            self: Pin<&mut Context>,
            metadata: &CxxString,
        ) -> Result<UniquePtr<CxxString>>;

        /// # Safety
        /// `context` must outlive the returned fragment. Its query lifecycle must
        /// remain exclusively owned by that fragment until execution finishes.
        unsafe fn make_fragment(context: Pin<&mut Context>) -> Result<UniquePtr<Fragment>>;

        fn build(self: Pin<&mut Fragment>, plan: &CxxString) -> Result<()>;
        fn run(self: Pin<&mut Fragment>) -> Result<()>;

        /// # Safety
        /// `out_stream_addr` must point to a writable ArrowArrayStream, and the
        /// stream must be drained or released before the fragment's context dies.
        unsafe fn result_to_arrow(self: Pin<&mut Fragment>, out_stream_addr: usize) -> Result<()>;

        /// Execute a serialized Substrait plan on the GPU, writing the results
        /// into the Arrow C Data Interface stream at `out_stream_addr` — the
        /// address (as `usize`) of a caller-owned `ArrowArrayStream` the caller
        /// releases per the Arrow ABI. `plan` binds to the C++ `const
        /// std::string&` and carries the protobuf-encoded `substrait::Plan`
        /// bytes. Bound as fallible: translation or execution failure surfaces as
        /// `Err(cxx::Exception)`.
        ///
        /// # Safety
        /// `out_stream_addr` must be the address of a valid, writable
        /// `ArrowArrayStream` that outlives this call; C++ writes the result
        /// stream through it. The safe [`sirius`](https://docs.rs/sirius) wrapper
        /// upholds this.
        unsafe fn execute_substrait(
            self: Pin<&mut Context>,
            plan: &CxxString,
            out_stream_addr: usize,
        ) -> Result<()>;
    }
}

pub use ffi::{Context, Fragment, make_context, make_context_from_config, make_fragment};
