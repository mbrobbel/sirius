//! Low-level `cxx` bindings to the Sirius C++ API.
//!
//! This crate is intentionally thin: it exposes the C++ types and free functions
//! declared in the `#[cxx::bridge]` module below and nothing else. Safe, idiomatic
//! wrappers live in the [`sirius`](https://docs.rs/sirius) crate.
//!
//! The bridge binds Sirius's **public C++ surface** (`src/include/sirius_ffi.hpp`):
//! RAII [`Context`], [`DataRepository`], and [`DataBatch`] handles held via
//! [`cxx::UniquePtr`]. The repository/batch handles are Sirius-owned adapters;
//! the third-party cuCascade types remain private to their C++ implementation.
//! The header is lightweight, so the bridge compiles without Sirius's internal
//! headers (cudf/rmm/duckdb/cuCascade). It is the seed of the public API
//! `libsirius` will expose; the bindings link whichever Sirius artifact provides
//! these symbols (the DuckDB extension today, a dedicated `libsirius` later —
//! see `build.rs`).
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
        include!("sirius_ffi.hpp");

        /// RAII handle to an initialized Sirius engine context.
        type Context;

        /// Opaque Sirius-owned handle to a native cuCascade data batch.
        type DataBatch;

        /// Opaque Sirius-owned handle to a native cuCascade shared data repository.
        type DataRepository;

        /// Create an empty data repository, owned by the returned `UniquePtr`.
        fn make_data_repository() -> UniquePtr<DataRepository>;

        /// Return the immutable native batch identifier.
        fn id(self: &DataBatch) -> u64;

        /// Transfer `batch` into `partition_idx`.
        fn push(
            self: Pin<&mut DataRepository>,
            batch: UniquePtr<DataBatch>,
            partition_idx: usize,
        ) -> Result<()>;

        /// Remove the next batch from `partition_idx`, returning a null
        /// `UniquePtr` when the partition is empty.
        fn pop_next(
            self: Pin<&mut DataRepository>,
            partition_idx: usize,
        ) -> Result<UniquePtr<DataBatch>>;

        /// Return the number of batches in `partition_idx`.
        fn size(self: &DataRepository, partition_idx: usize) -> Result<usize>;

        /// Return the number of batches across all partitions.
        fn total_size(self: &DataRepository) -> Result<usize>;

        /// Return the current number of partitions.
        fn num_partitions(self: &DataRepository) -> Result<usize>;

        /// Grow the repository to `new_num_partitions`.
        fn set_num_partitions(
            self: Pin<&mut DataRepository>,
            new_num_partitions: usize,
        ) -> Result<()>;

        /// Construct an initialized [`Context`] from built-in defaults, owned by
        /// the returned `UniquePtr`.
        fn make_context() -> Result<UniquePtr<Context>>;

        /// Construct an initialized [`Context`] from the YAML config file at
        /// `config_path`, owned by the returned `UniquePtr`. `config_path` binds
        /// to the C++ `const std::string&` parameter.
        fn make_context_from_config(config_path: &CxxString) -> Result<UniquePtr<Context>>;

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

pub use ffi::{
    Context, DataBatch, DataRepository, make_context, make_context_from_config,
    make_data_repository,
};

#[cfg(test)]
mod tests {
    use super::make_data_repository;

    /// Exercises the adapter and cuCascade linkage without constructing a GPU context.
    #[test]
    fn empty_repository_round_trip() {
        let mut repository = make_data_repository();
        assert_eq!(repository.num_partitions().unwrap(), 1);
        assert_eq!(repository.total_size().unwrap(), 0);
        assert_eq!(repository.size(0).unwrap(), 0);
        assert!(repository.pin_mut().pop_next(0).unwrap().is_null());

        repository.pin_mut().set_num_partitions(2).unwrap();
        assert_eq!(repository.num_partitions().unwrap(), 2);
        assert!(repository.size(2).is_err());
    }
}
