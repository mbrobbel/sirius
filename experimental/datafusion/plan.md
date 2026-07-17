# DataFusion integration POC handoff

## Direction

Build the DataFusion integration in `experimental/datafusion`, alongside the
existing StarRocks experiment. Both integrations have the same central path:

```text
external planner -> Substrait bytes -> Sirius context -> Arrow batches -> external consumer
```

The comparison target is
[`libcudf-datafusion-benchmarks`](https://github.com/mbrobbel/libcudf-rs/tree/main/libcudf-datafusion-benchmarks).
Keep its dataset preparation, query files, warmup behavior, timing loop, JSON
results, and reporting. Integrate Sirius at the runner's query-execution seam
instead of forking the benchmark harness.

The DataFusion POC should keep its eager engine host inside the
`sirius-datafusion` crate for now. Extracting a common executor with StarRocks
is later work and must not require changes under `experimental/starrocks` in
the initial milestones.

## Reuse from the StarRocks experiment

`experimental/starrocks/src/engine.rs` already provides the important generic
pieces:

- a dedicated thread that owns the `!Send`/`!Sync` `SiriusContext`;
- fail-fast context initialization from an optional config file;
- owned request and response channels;
- serialized Substrait input and owned Arrow `RecordBatch` output;
- safe transfer of results away from the engine thread; and
- ordered shutdown of the process-global Sirius context.

Adapt that pattern locally in `sirius-datafusion`. Its initial engine API only
needs to accept Substrait bytes and return Arrow batches:

```rust
pub fn start(config: Option<PathBuf>) -> Result<SiriusExecutor, String>;
pub fn execute(&self, plan: Vec<u8>) -> Result<Vec<RecordBatch>, String>;
```

DataFusion supplies the bytes produced by `datafusion-substrait`. Keep
StarRocks-specific fragment types, result encoding, and RPC handling out of
this crate. A later extraction can replace the local executor without changing
the DataFusion-facing API.

The two integrations then differ only at their edges:

| Layer | StarRocks | DataFusion |
|---|---|---|
| Plan source | StarRocks fragment translator | DataFusion SQL and logical plan |
| Substrait preparation | `starrocks-plan-translator` | `datafusion-substrait` plus table binding |
| Sirius execution | Existing StarRocks executor | DataFusion-local eager executor |
| Result consumer | StarRocks result encoder/store | DataFusion `MemTable`, later an `ExecutionPlan` |

Use a standalone crate, commit the lockfile, and feature gate the GPU-linked
executor so pure plan tests can run without a Sirius build or GPU. Match the
DataFusion and Arrow versions used by `libcudf-datafusion-benchmarks` at the
public boundary. The `sirius` crate exposes selectable Arrow 58 and 59 result
features: this integration selects Arrow 58 directly, while Arrow 59 remains
the default for the existing StarRocks consumer.

## Relationship to PR #914

[PR #914](https://github.com/sirius-db/sirius/pull/914) proposes the longer-term
public API shared by database integrations: a non-blocking `StreamSession`,
bounded input and output channels, `push`/`close_input`, `pull`/`wait`,
backpressure, cancellation, and streaming Sirius source/sink operators.

The eager `Vec<u8> -> Vec<RecordBatch>` executor is an appropriate first POC,
but it should be treated as a compatibility layer. Do not design a separate
DataFusion-specific streaming or context-lifetime API. Once `StreamSession` is
implemented, back the shared executor with it and let both StarRocks and
DataFusion use the same lifecycle, flow-control, and telemetry behavior.

A future DataFusion `SiriusExec` can then:

- push DataFusion input batches into a Sirius session when the plan has an
  external input;
- close the input stream on DataFusion end-of-stream;
- expose Sirius output as a DataFusion `RecordBatchStream`; and
- map DataFusion cancellation and errors onto the common session.

This should be a later milestone. `SiriusContext::execute_substrait` currently
materializes the Arrow stream eagerly, which is sufficient for correctness and
initial benchmarks.

## DataFusion-specific work

Create a standalone project with a small surface:

```text
experimental/datafusion/
├── Cargo.toml
├── README.md
└── src/
    ├── lib.rs        # benchmark-facing library boundary
    ├── main.rs       # CLI and benchmark loop
    ├── plan.rs       # SQL -> Substrait and table binding
    └── compare.rs    # schema and result comparison
```

Pin `datafusion` and `datafusion-substrait` to the same exact release or commit.
Do not use `datafusion-ffi` for this phase: DataFusion and Sirius are compiled
into one process, while Substrait and the Arrow C Data Interface already form a
narrow boundary.

DataFusion currently produces a Substrait `NamedTable`, while standalone Sirius
needs a resolvable `LocalFiles` read. Isolate the rewrite in `plan.rs`, support a
map of table names to paths, traverse every relation input, and return clear
errors for missing, duplicate, or unsupported bindings. Do not retain the
Q1-specific unary relation walker from the first experiment.

For correctness, execute the same SQL and Parquet files with DataFusion and
Sirius and compare:

- Arrow schema, including nullability and decimal precision/scale;
- row count; and
- canonicalized rows, sorting only when SQL does not define ordering.

Start with TPC-H Q1 and Q6 at SF10, then SF100. Register Sirius output as a
DataFusion `MemTable` to prove DataFusion can consume the result. Also test one
unsupported plan and require an explicit error rather than fallback.

## Benchmark protocol

Keep context initialization and plan construction outside the query execution
timer, but record separate planning, Substrait/binding, Sirius execution, result
transfer, and end-to-end timings. Emit JSON or CSV with both revisions, GPU,
dataset, query, scale factor, and Sirius config.

Use release builds. Run one controlled cold-cache iteration per engine and at
least five alternating warm iterations. Report median and fastest warm time and
state whether result materialization is included. Benchmark buffered I/O,
`O_DIRECT`, and prefetch-cache modes as separate configurations.

Findings worth retaining as diagnostic baselines:

- SF10 warm Q1: about 0.29 seconds for DataFusion and 0.32 seconds for buffered
  Sirius.
- SF100 warm Q1 after the prefetch-range fix: about 3.4 seconds for DataFusion
  and 2.0 seconds for buffered Sirius.
- Unused Parquet range derivation previously consumed 16--21 seconds during
  SF100 task creation when no prefetch cache existed. The fix is on
  `perf/skip-unused-prefetch-ranges`.
- Warm SF100 Q1 with default `O_DIRECT` remained about 12.4 seconds, so I/O mode
  cannot be an implicit benchmark detail.

These are not publication-quality numbers; replace them with saved runner
output and Sirius telemetry.

## Suggested milestones

1. Add the benchmark-compatible `sirius-datafusion` library boundary and local
   eager executor without changing StarRocks.
2. Add Q1 correctness at SF10 using the libcudf benchmark dataset and query
   layout.
3. Integrate Sirius as another engine in the libcudf benchmark runner, then add
   Q6, SF100, controlled benchmarks, and saved telemetry.
4. Extract common executor code only after both integrations have proven the
   eager boundary.
5. Replace eager execution with the common stream session from PR #914.
6. Add `SiriusExec` only after the shared streaming and cancellation contracts
   are stable.

The local crate now contains milestone 1's benchmark-compatible API, eager
executor, recursive table binding, and standalone CLI. Pure planning tests run
without the engine feature; the ignored hardware integration test exercises a
real DataFusion query through Sirius when a CUDA GPU is available.

The POC is complete through milestone 3 when it runs from the Sirius Pixi
environment, pins all revisions, produces matching SF10/SF100 results, records
controlled timings, and requires no modifications to a DataFusion checkout.
