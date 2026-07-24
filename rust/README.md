# Sirius Rust bindings

Crates for driving [Sirius](https://github.com/sirius-db/sirius) from Rust
(sirius-db/sirius #835).

| Crate | Role |
|-------|------|
| [`sirius-sys`](crates/sirius-sys) | Low-level [`cxx`](https://cxx.rs) bindings to Sirius's public C++ FFI (`src/include/sirius_ffi.hpp`). |
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
needs a GPU, and the runtime loader must find the linked library; until a
dedicated `libsirius` is installed, point it at the build tree:

```bash
LD_LIBRARY_PATH="$PWD/build/release/extension/sirius:$LD_LIBRARY_PATH" \
  pixi run cargo test --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

## Native data boundary

`SiriusContext::create_data_repository()` creates a safe Rust
`DataRepository<'context>` backed by cuCascade's thread-safe
`shared_data_repository`. `DataBatch<'context>` is an owned Rust value wrapping
cuCascade's shared native `data_batch`; pushing consumes the Rust value and
popping returns a new owned value. Both lifetimes are tied to the context whose
memory spaces back native batch representations.

Neither cuCascade C++ type appears in the cxx bridge. The public
`sirius_ffi.hpp` contains only Sirius-defined PIMPL handles, while
`ffi/cucascade_adapter.hpp` is the native-only conversion point. Future
streaming sync/source integration should share the adapter's native repository
with the operator and convert operator-produced batches there. Transport,
session scheduling, and push/pull behavior intentionally remain outside this
foundational layer.

## Linkage

`build.rs` discovers the Sirius artifact under `$SIRIUS_BUILD_DIR` (default
`build/release`) and links **one self-contained library** — no hand-maintained
dependency list:

- **default** → `libsirius.so` (shared; pulls its deps via `DT_NEEDED`). Until a
  real `libsirius.so` exists, `build.rs` symlinks the DuckDB extension
  (`sirius.duckdb_extension`) to it.
- **`--features static`** → `libsirius.a` (self-contained, no runtime deps — the
  fully static vcpkg build). Requires that bundled archive to exist.

`build.rs` only needs `src/include` to compile the shim, because the bound
surface is the lightweight `sirius_ffi.hpp`. That header is the seed of the public
C++ API `libsirius` will expose; today it is compiled into the DuckDB extension,
which the bindings link until a dedicated `libsirius` ships (at which point the
symlink stopgap is no longer used).

## Environment

- `SIRIUS_BUILD_DIR` — Sirius build tree (default `build/release`).
- `CONDA_PREFIX` — set by `pixi`; used to find the headers and the shared lib's deps.
- `CARGO_NET_GIT_FETCH_WITH_CLI=true` — only on machines whose git config rewrites
  `https://github.com/` to SSH (the telemetry crate's `quent` git dep otherwise
  fails libgit2's ssh-agent path). CI is unaffected.
