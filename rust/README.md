# Sirius Rust bindings

Crates for driving [Sirius](https://github.com/sirius-db/sirius) from Rust
(sirius-db/sirius #835).

| Crate | Role |
|-------|------|
| [`sirius-sys`](crates/sirius-sys) | Low-level [`cxx`](https://cxx.rs) bindings to Sirius's public C++ API (`include/sirius/ffi.hpp`). |
| [`sirius`](crates/sirius) | Safe, idiomatic wrapper over `sirius-sys`. |

(The `telemetry/*` crates are unrelated — Rust linked *into* the C++ extension via
CMake/Corrosion, the opposite direction.)

## Building & testing

The crates compile a small cxx shim against Sirius's headers and **link a Sirius
library artifact**, so build Sirius first, then use cargo:

```bash
pixi run make                       # builds and installs Sirius, then builds the wrapper
# build + link the tests (no GPU needed):
pixi run cargo test --no-run --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

`SiriusContext::new()` brings up a **fully initialized** engine (it calls the C++
`initialize()`, which does GPU bring-up) and tears it down on drop — pure RAII via
`cxx::UniquePtr`, no uninitialized state. So **running** the proof-of-life test
needs a GPU, and the runtime loader must find the installed library:

```bash
SIRIUS_PREFIX="$PWD/build/release/install" \
LD_LIBRARY_PATH="$PWD/build/release/install/lib:$LD_LIBRARY_PATH" \
  pixi run cargo test --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

## Linkage

`build.rs` uses installed public headers and a library under `SIRIUS_PREFIX`.
Without an explicit prefix it checks the active conda environment, then
`build/release/install`. It never links or creates symlinks to a DuckDB extension.

- **default**: `libsirius.so`, with dependencies recorded in `DT_NEEDED`.
- **`--features static`**: combined `libsirius.a`, retaining registration objects
  through whole-archive linkage. Platform and driver libraries come from the
  installed CMake link metadata; redistributable dependencies are in the archive.

For a standalone library build, install it first:

```bash
pixi run cmake --install build/sirius --prefix "$PWD/build/install" --component sirius_library
SIRIUS_PREFIX="$PWD/build/install" pixi run cargo test --no-run \
  --manifest-path rust/Cargo.toml -p sirius -p sirius-sys
```

Cargo does not propagate raw linker arguments across crates. Binaries using the
static feature must emit these in their own `build.rs` (the `sirius` crate does so
for its tests), or supply equivalent `RUSTFLAGS`:

```rust
println!("cargo:rustc-link-arg=-Wl,--allow-multiple-definition");
println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol=InitializeInjectionNvtx2");
println!("cargo:rustc-link-arg=-Wl,--export-dynamic-symbol=dlopen");
```

## Environment

- `SIRIUS_PREFIX` — installed Sirius prefix (replaces `SIRIUS_BUILD_DIR`).
- `CONDA_PREFIX` — set by `pixi`; used for package and dependency discovery.
- `CARGO_NET_GIT_FETCH_WITH_CLI=true` — only on machines whose git config rewrites
  `https://github.com/` to SSH (the telemetry crate's `quent` git dep otherwise
  fails libgit2's ssh-agent path). CI is unaffected.
