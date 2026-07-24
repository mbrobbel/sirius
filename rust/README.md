# Sirius Rust bindings

Crates for driving [Sirius](https://github.com/sirius-db/sirius) from Rust
(sirius-db/sirius #835).

| Crate | Role |
|-------|------|
| [`sirius-sys`](crates/sirius-sys) | Low-level [`cxx`](https://cxx.rs) bindings to Sirius's public C-ABI (`src/include/sirius_ffi.h`). |
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

## Synchronous streaming compatibility

The safe crate exposes the future-facing stream-session lifecycle while the
native streaming operators are still under development:

```rust
let plan = sirius::SubstraitPlan::decode(&substrait_plan)?;
let input_stream = plan.input_streams()[0];
let output_stream = plan.output_streams()[0];

// Match these opaque IDs with transport metadata retained by the embedding
// runtime during translation, then move the plan into its engine session.
let mut session = context.create_stream_session(plan)?;
session.push_batch(input_stream, input_batch)?;
session.end_stream(input_stream)?;
let output = session.pull_batch_sync(output_stream)?.unwrap();
```

This compatibility path is intentionally synchronous and single-shot. The plan
must contain exactly one `ReadRel`; the session substitutes the one pushed Arrow
batch for that read as an in-memory Substrait `VirtualTable`, executes the
existing Substrait API once, and requires exactly one result batch. Additional
input reads, input batches, result batches, or unsupported Arrow input types
return `StreamSessionError`. Stream identifiers are discovered from the plan,
not configured separately by the caller. `SubstraitPlan` exposes them as opaque
correlation keys before session creation. Transport-specific exchange details
remain in the embedding runtime and are matched to those keys; Sirius does not
interpret or retain them. The compatibility plan exposes one input and one
output; native streaming plans can expose one input per source and multiple
outputs, including one per partition of a partitioned sink.

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
surface is the lightweight `sirius_ffi.h`. That header is the seed of the public
C++ API `libsirius` will expose; today it is compiled into the DuckDB extension,
which the bindings link until a dedicated `libsirius` ships (at which point the
symlink stopgap is no longer used).

## Environment

- `SIRIUS_BUILD_DIR` — Sirius build tree (default `build/release`).
- `CONDA_PREFIX` — set by `pixi`; used to find the headers and the shared lib's deps.
- `CARGO_NET_GIT_FETCH_WITH_CLI=true` — only on machines whose git config rewrites
  `https://github.com/` to SSH (the telemetry crate's `quent` git dep otherwise
  fails libgit2's ssh-agent path). CI is unaffected.
