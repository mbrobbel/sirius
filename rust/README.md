# Sirius Rust bindings

Crates for driving [Sirius](https://github.com/sirius-db/sirius) from Rust
(sirius-db/sirius #835).

| Crate | Role |
|-------|------|
| [`sirius-sys`](crates/sirius-sys) | Raw bindings and native library linking for the public C ABI. |
| [`sirius`](crates/sirius) | Safe configuration and context wrappers over `sirius-sys`. |
| [`sirius-ffi`](crates/sirius-ffi) | Existing C++ FFI bridge, execution wrappers, and object-store bindings. |
| [`sirius-substrait`](crates/sirius-substrait) | Prototype Substrait consumer using the public plan API. |

(The `telemetry/*` crates are unrelated — Rust linked *into* the C++ extension via
CMake/Corrosion, the opposite direction.)

## Configuration

Build a validated, immutable configuration without initializing GPUs:

```rust
use sirius::{ConfigError, ContextConfigBuilder};

fn load_config() -> Result<sirius::ContextConfig, ConfigError> {
    ContextConfigBuilder::from_yaml("sirius.yaml")?.build()
}
```

Use `ContextConfigBuilder::new()?.build()?` for built-in defaults. Both the
builder and configuration offer `try_clone()` to create independent handles
sharing immutable settings. The file is read only by `from_yaml()`; later file
changes do not affect a loaded builder. Configuration errors retain their
I/O, YAML syntax, or invalid-setting category in `ConfigError`.

Configuration and context construction call the C ABI directly. Building `sirius`
and `sirius-sys` requires no C++ compilation. The configuration and context types
are neither `Send` nor `Sync`.

## Public context

Construct an engine from a configuration; dropping the handle releases it:

```rust
use sirius::{Context, ContextConfigBuilder};

fn create_engine() -> Result<Context, Box<dyn std::error::Error>> {
    let config = ContextConfigBuilder::from_yaml("sirius.yaml")?.build()?;
    Ok(Context::new(&config)?)
}
```

`ContextError` reports initialization, allocation, and unexpected native failures. The configuration
can be dropped after construction. Only one active engine context per process is
supported, including the existing `SiriusContext` execution wrapper and other Sirius
integrations. This restriction is not enforced; callers must ensure context lifetimes
do not overlap. Forking with an active context is unsupported. The public `Context`
also executes immutable plans built through the prototype public plan builder.
The existing execution API remains in `sirius_ffi::SiriusContext`.

Run the context example with a YAML file and the build-tree shared library:

```bash
LD_LIBRARY_PATH="$PWD/build/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  pixi run cargo run --manifest-path rust/Cargo.toml -p sirius --example context -- sirius.yaml
```

## Substrait prototype

The [`sirius-substrait` consumer](crates/sirius-substrait/README.md) translates a
small subset of Substrait into public plan-builder handles and executes them on
`Context`. See its README for supported relations and the GPU example.

## Existing execution and object-store API

Execution wrappers moved from `sirius` to `sirius-ffi`:

```rust
use sirius_ffi::SiriusContext;
```

The raw C++ bridge previously exposed by `sirius-sys` is now available through
`sirius_ffi::bridge` and re-exported from the `sirius-ffi` crate root, including
`ObjectStoreConfigArgs` and its factories. StarRocks and Doris use this crate.
This API remains supported without deprecation warnings while the public C API
adds execution support. Its contexts share the same one-active-context-per-process
limit with `sirius::Context`.

## Building & testing

All four crates link a Sirius library artifact. Only `sirius-ffi` compiles a C++20
bridge and requires a matching C++ toolchain. Build Sirius first, then use cargo:

```bash
pixi run make                       # builds the Sirius extension (+ artifact + headers)
# build + link the tests (no GPU needed):
pixi run cargo test --no-run --manifest-path rust/Cargo.toml -p sirius -p sirius-sys -p sirius-ffi -p sirius-substrait
```

The context lifecycle and `sirius-ffi` execution tests need a GPU; the configuration
tests do not initialize the engine. Point the runtime loader at the build-tree library:

```bash
LD_LIBRARY_PATH="$PWD/build/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  pixi run cargo test --manifest-path rust/Cargo.toml -p sirius -p sirius-sys -p sirius-ffi -p sirius-substrait
```

## Documentation

Build with the isolated Pixi docs environment:

```bash
pixi run -e docs docs-rust
```

Open `build/docs/rust-target/doc/sirius/index.html`. The task also publishes the
`sirius-sys` reference and sets `DOCS_RS` for the documentation build.
The `sirius-ffi` and `sirius-substrait` references are excluded from publication.

Generate the `sirius` API reference without building or linking `libsirius`:

```bash
DOCS_RS=1 cargo doc --locked --manifest-path rust/Cargo.toml -p sirius --lib --no-deps
```

Open `rust/target/doc/sirius/index.html`. This requires only a Rust toolchain,
with no Sirius build, CUDA toolkit, GPU, or C++ compiler.
`DOCS_RS` skips Sirius's native bridge compilation and library lookup; use it
only for documentation, leaving it unset for normal builds and tests.

## Linkage

`build.rs` discovers the Sirius artifact under `$SIRIUS_BUILD_DIR` (default
`build/release`) and links one Sirius library:

- **default** → `libsirius.so` (shared; pulls its deps via `DT_NEEDED`).
- **`--features static`** → an externally bundled `libsirius.a`. The current
  CMake build does not produce this bundle; its raw archive is unsupported.
  With a suitable bundle, the C++ standard library is linked separately. This feature alone does
  not produce a fully static executable. Applications must also supply any native
  dependencies not bundled in their archive (including the NVIDIA driver libraries).
  Bundles containing duplicate DuckDB symbols require
  `-C link-arg=-Wl,--allow-multiple-definition` on the final binary; this flag is
  applied to these crates' own targets but not propagated to dependent applications.

`sirius-sys/build.rs` supplies native library linking for all four crates.
`sirius-ffi/build.rs` compiles the execution bridge against the public `include`
directory. Configuration and context use the C declarations in
`sirius-sys/src/c_api.rs`. Neither path includes internal engine headers.

## Environment

- `DOCS_RS` — when set, skip Sirius's native build and link steps for documentation.
- `SIRIUS_BUILD_DIR` — Sirius build tree (default `build/release`).
- `CONDA_PREFIX` — set by `pixi`; used to find the shared library's dependencies.
- `CARGO_NET_GIT_FETCH_WITH_CLI=true` — only on machines whose git config rewrites
  `https://github.com/` to SSH (the telemetry crate's `quent` git dep otherwise
  fails libgit2's ssh-agent path). CI is unaffected.
