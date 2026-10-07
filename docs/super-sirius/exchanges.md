# Native exchanges

An exchange moves a fragment's GPU batches between Sirius processes. Each embedding
`sirius::ffi::Context` can own one NIXL agent. The caller chooses agent names, exchanges
opaque peer metadata, and starts the fragments; Sirius handles partitioning, transfer,
receive, and end-of-stream tracking.

The source and sink inherit the repository and lifecycle mechanics described in
[Streaming Sessions](streaming-sessions.md). They appear in physical plans as
`EXCHANGE_SOURCE` and `EXCHANGE_SINK`. An exchange source is scheduled like a scan and
wakes when a received batch arrives. An exchange sink terminates the GPU pipeline;
the fragment also waits for network completion before releasing its query lifecycle.

## Plan format

The shared schema is [exchange.proto](../../proto/sirius/exchange/v1/exchange.proto).
Its versioned messages are carried in Substrait `Any` values:

| Boundary | Substrait location | Metadata |
|---|---|---|
| Source | `ReadRel.extension_table.detail` | `sirius.exchange.v1.ExchangeSource` |
| Sink | `ExchangeRel.advanced_extension.enhancement` | `sirius.exchange.v1.ExchangeSink` |
| Destination | `ExchangeRel.targets[i].extended` | `sirius.exchange.v1.ExchangeDestination` |

The type URL prefix is `type.googleapis.com/`. Query and fragment IDs retain both
64-bit halves. Incoming routes are identified by query ID, receiver fragment ID,
and exchange ID. The source declares its expected sender IDs and flat scalar
schema in `ReadRel.base_schema`. A destination's `peer_id` is the agent name returned
when its bootstrap metadata is registered.

A fragment has one `PlanRel.root`. Its root may be an `ExchangeRel` sink, with one
explicit destination per partition. Supported routing is:

- Gather: one partition, `single_target` with a constant zero bucket.
- Hash: `scatter_by_fields` with direct references to output columns.
- Broadcast: `broadcast`, with every target receiving the complete output.

Nested sinks, computed partition expressions, round-robin routing, and additional
enhancements on exchange boundaries are rejected. Hash key indices are checked
against the bound output schema. The existing streaming sink normalizes supported key types before
hashing so independent senders use the same representation.

Before DuckDB imports a fragment, Sirius extracts the exchange declarations,
replaces source reads with declared stream views, and removes the root exchange
sink. The normal plan generator then builds the subtree and installs exchange
operators at its boundaries. The DuckDB Substrait importer does not need changes.

## Embedding lifecycle

1. Construct a context with a suitable GPU and host memory configuration.
2. Call `enable_exchange(agent_name, staging_bytes, timeout_ms)`.
3. Export `exchange_metadata()` and give it to the other processes through the
   caller's coordinator. Register their blobs with `add_exchange_peer()`.
4. Create a `Fragment` and call `build(plan_bytes)`. Exchange plans declare their
   streams automatically; manual declarations cannot be mixed with them.
5. Wait until the participating receivers have built their fragments, then call
   `run()` on each process. Repeat this readiness barrier for every query; a peer
   does not retain notifications addressed to a fragment it has not attached.
6. Result fragments expose Arrow through `result_to_arrow()`. Intermediate fragments
   complete after sending their output. Drop the fragment before reusing its context.

An exchange context permits only one attached exchange fragment at a time.
Ordinary C++ fragments may be built before another fragment runs. Each `build()`
and `run()` owns its scoped DuckDB transaction and query lifecycle; building a
fragment does not leave either open while waiting for the readiness barrier.
During `run()`, the context remains occupied until all transfers and end-of-stream
acknowledgments finish, including peer-progress waits up to the configured timeout.

The Rust API provides `SiriusContext::fragment()` and `SiriusFragment`. Its mutable
borrow keeps the context alive and prevents concurrent use while a fragment exists.
Use separate processes for concurrent contexts because the engine currently has
process-global GPU state.

Registered staging buffers are reused across queries. Data travels as packed cuDF
columns, with metadata describing their layout. Transfer acknowledgments govern
buffer reuse; sender-specific end-of-stream messages close a source only after all
declared senders finish. Failures poison streams and wake waiting consumers.
Fragment teardown stops transport work before releasing its borrowed operators.

A failed or cancelled transport cannot be reused. Any failed exchange `run()`
cancels transport, including errors detected before GPU execution starts. Create
a new context after an exchange failure; potentially active transfer buffers remain alive until the old
agent is destroyed. This keeps cancellation bounded without reusing buffers that
NIXL may still reference. Successful fragments can reuse the context normally.

The timeout starts when `run()` begins. Time spent between `build()` and `run()`
does not count. It bounds waits for peer progress, including an expected sender
that is still computing; choose a timeout long enough for remote computation.
A sender performing local computation with no outstanding network request does
not time out. This version does not exchange heartbeats.

## Build and test

NIXL is included in every Sirius build. The complete GPU integration harness
can be built and run with:

```bash
pixi run make exchange-test
```

This target configures a standalone Sirius build in `build/exchange`, builds
the shared library, and runs the Rust process harness.
Use `EXCHANGE_BUILD_DIR=<path>` to choose another build directory. CMake fetches
the pinned NIXL 1.5.0 source and builds its native SDK as
static archives with the UCX backend built in. Pixi provides Meson, ASIO,
tomlplusplus, and UCX 1.20.1. The development build links Pixi's shared UCX
libraries; activating Pixi supplies their runtime dependencies.

The vcpkg build uses the repository's NIXL and UCX overlay ports. Both produce
static archives linked by the Sirius targets. The UCX port enables TCP, shared
memory, and CUDA transports. InfiniBand and RoCE support are deferred.

```bash
pixi run -e vcpkg cmake -S duckdb --preset vcpkg-release
pixi run -e vcpkg cmake --build build/vcpkg-release --target sirius_shared
```

The static ports require no runtime NIXL or UCX plugins. The CUDA driver and
NVML remain host dependencies.

To configure and build manually, including the native parser/operator tests:

```bash
pixi run cmake -S . -B build/exchange -G Ninja -DCMAKE_BUILD_TYPE=Release \
  -DSIRIUS_BUILD_TESTS=ON -DSIRIUS_BUILD_S3_TESTS=OFF
pixi run cmake --build build/exchange --target sirius_shared sirius_unittest
pixi run build/exchange/test/cpp/sirius_unittest \
  '[exchange_plan],[exchange_operator],[sirius_ffi]'
```

To rerun the Rust harness against an existing build:

```bash
SIRIUS_BUILD_DIR="$PWD/build/exchange" \
LD_LIBRARY_PATH="$PWD/build/exchange:${LD_LIBRARY_PATH:-}" \
  pixi run cargo test --locked --manifest-path rust/Cargo.toml -p sirius \
  --test exchange exchange_end_to_end -- --ignored --nocapture
```

The native runner accepts a smaller memory configuration through
`SIRIUS_TEST_INTEGRATION_CONFIG` when the GPU is shared. The Rust harness uses
128 MiB GPU pools per worker and needs enough free memory for five workers plus
their CUDA contexts.

## Scope

This protocol connects Sirius peers. It does not implement StarRocks' native batch
transport. Peer bootstrap and distributed execution orchestration remain the embedding
caller's responsibility.

The local Rust integration tests use a single GPU across separate worker processes.
They validate rows, routing, and lifecycle behavior. Multi-host bandwidth and
registration cost measurements require a separate setup. EFA support is outside
the initial TCP/shared-memory/CUDA dependency configuration.
