# Sirius Rust bindings

Crates for driving [Sirius](https://github.com/sirius-db/sirius) from Rust
(sirius-db/sirius #835).

| Crate | Role |
|-------|------|
| [`sirius-sys`](crates/sirius-sys) | Low-level [`cxx`](https://cxx.rs) bindings to Sirius's public C-ABI (`include/sirius/ffi.hpp`). |
| [`sirius`](crates/sirius) | Safe, idiomatic wrapper over `sirius-sys`. |

(The `telemetry/*` crates are unrelated — Rust linked *into* the C++ extension via
CMake/Corrosion, the opposite direction.)

## Building & testing

The crates compile a small cxx shim against Sirius's headers and **link a Sirius
library artifact**, so build Sirius first, then use cargo:

```bash
pixi run make                       # builds the Sirius extension (+ artifact + headers)
# build + link the tests (no GPU needed):
pixi run cargo test --no-run --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

`SiriusContext::new()` brings up a **fully initialized** engine (it calls the C++
`initialize()`, which does GPU bring-up) and tears it down on drop — pure RAII via
`cxx::UniquePtr`, no uninitialized state. So **running** the proof-of-life test
needs a GPU, and the runtime loader must find the linked library. For the
DuckDB extension build, point it at the build tree:

```bash
LD_LIBRARY_PATH="$PWD/build/release/extension/sirius:$LD_LIBRARY_PATH" \
  pixi run cargo test --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

### Exchange integration test

Build Sirius with NIXL enabled and run the complete process-based GPU test:

```bash
pixi run make exchange-test
```

The target creates a standalone shared-library build in `build/exchange` with
`SIRIUS_ENABLE_NIXL=ON`, then points Cargo and the runtime loader at that directory.
Override it with `EXCHANGE_BUILD_DIR=<path>`. CMake fetches NIXL 1.5.0 and builds
its native SDK with the UCX backend built in; Pixi supplies the build tools and
UCX 1.20.1. The vcpkg build uses static NIXL and UCX overlay ports supporting
TCP, shared memory, and CUDA. See the [native exchange guide](../docs/super-sirius/exchanges.md)
for manual build commands and transport scope.

The target leaves NIXL enabled in the selected build directory's CMake cache. To disable it
for subsequent builds, run
`pixi run cmake -S . -B build/exchange -DSIRIUS_ENABLE_NIXL=OFF`.

The coordinator generates Parquet inputs and Substrait plans, launches one
engine per process with 128 MiB of GPU memory and 512 MiB of host memory,
exchanges opaque NIXL metadata through temporary files, and
starts each query after every fragment is built. Gather, hash, and broadcast
cover three senders (including an empty sender), nullable values, duplicate rows,
and batches larger than the 1 MiB staging buffer. Empty senders use typed
Substrait virtual tables; nonempty senders read Parquet. A second query on each context
filters received rows. Additional cases check all-empty input, mismatched input
schemas, and missing-sender timeouts. Receiver `LIMIT 1`, `LIMIT 0`, and constant
false filters check that early completion drains every sender and leaves the
context ready for an unrestricted query. The timeout case delays the start
barrier to verify that idle deadlines begin at `run()`. Dropping a built
fragment before `run()` checks that subsequent exchange use requires a new
context. Every child has a deadline and is killed
and reaped if its test fails. The harness also checks that a fragment rejects a
second `run()` after either success or failure. Set
`SIRIUS_EXCHANGE_KEEP_ARTIFACTS=1` to retain each scenario's plans, results, and
worker logs; their directory paths are printed during the run.

The Rust entry points are `SiriusContext::enable_exchange`,
`exchange_metadata`, `add_exchange_peer`, and `fragment`. A fragment exclusively
borrows its context until dropped; call `run()` and then `result()` for result
fragments. Intermediate fragments finish with `run()`. The shared
`sirius-exchange-proto` crate generates the versioned metadata messages placed
in Substrait exchange boundaries. Successful contexts can execute subsequent
fragments; a failed or abandoned exchange requires a new context.

Successful queries reuse the same agent and staging buffers. The native exchange
guide describes the bootstrap and protocol lifecycle.

## Documentation

Build with the isolated Pixi docs environment:

```bash
pixi run -e docs docs-rust
```

Open `build/docs/rust-target/doc/sirius/index.html`. This task installs the Rust
and C++ toolchains and sets `DOCS_RS` for the documentation build.

Generate the `sirius` API reference without building or linking `libsirius`:

```bash
DOCS_RS=1 cargo doc --locked --manifest-path rust/Cargo.toml -p sirius --lib --no-deps
```

Open `rust/target/doc/sirius/index.html`. This requires a Rust toolchain and a C++
compiler for the `cxx` dependency, but no Sirius build, CUDA toolkit, or GPU.
`DOCS_RS` skips Sirius's native bridge compilation and library lookup; use it
only for documentation, leaving it unset for normal builds and tests.

## Linkage

`build.rs` discovers the Sirius artifact under `$SIRIUS_BUILD_DIR` (default
`build/release`). It searches both the build root and `extension/sirius`, covering
standalone and DuckDB extension builds:

- **default** → `libsirius.so`, whose native dependencies are recorded in
  `DT_NEEDED`. Older builds without this library fall back to a symlink to
  `sirius.duckdb_extension`.
- **`--features static`** → `libsirius.a`. The current archive has transitive
  dependencies represented by CMake's `sirius::sirius_static` target. Cargo does
  not consume that metadata, so this feature alone cannot link the current
  static library. Use the shared library for Rust consumers.

`build.rs` only needs `include` to compile the shim, because the bound
surface is the lightweight public C++ header `sirius/ffi.hpp`.

## Environment

- `DOCS_RS` — when set, skip Sirius's native build and link steps for documentation.
- `SIRIUS_BUILD_DIR` — Sirius build tree (default `build/release`).
- `CONDA_PREFIX` — set by `pixi`; used to find the headers and the shared lib's deps.
- `CARGO_NET_GIT_FETCH_WITH_CLI=true` — only on machines whose git config rewrites
  `https://github.com/` to SSH (the telemetry crate's `quent` git dep otherwise
  fails libgit2's ssh-agent path). CI is unaffected.
