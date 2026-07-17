# Sirius DataFusion integration

This crate implements the benchmark-facing DataFusion integration described in [`plan.md`](plan.md).
It translates optimized DataFusion logical plans to Substrait, binds registered Parquet
`ListingTable` inputs to local files, executes plans on a dedicated Sirius engine thread, and
returns DataFusion `RecordBatch` values.

## Dependency boundary

DataFusion and `datafusion-substrait` are pinned to crates.io release 53.1.0 and Arrow 58.2.0. These
are the versions currently locked by
[`libcudf-datafusion-benchmarks`](https://github.com/mbrobbel/libcudf-rs/tree/main/libcudf-datafusion-benchmarks),
so this crate can be added to that workspace without introducing a second DataFusion type graph.
The `sirius` dependency selects its Arrow 58 result feature, so results use DataFusion's
`RecordBatch` type directly. Sirius retains Arrow 59 as its default for the existing StarRocks
consumer.

The local `sirius` dependency is optional and enabled by the `sirius-engine` default feature. This
keeps planning and table-binding work testable without a compiled Sirius engine or a GPU:

```bash
pixi run cargo check --manifest-path experimental/datafusion/Cargo.toml --no-default-features
```

Before checking or running the engine-enabled build, build Sirius from the repository root:

```bash
pixi run make
pixi run cargo check --manifest-path experimental/datafusion/Cargo.toml
```

The standard `build/release` tree is discovered automatically. If `SIRIUS_BUILD_DIR` is set, use an
absolute path because Cargo build scripts do not resolve it relative to the invoking shell.

The standalone CLI takes one or more `NAME=PATH` Parquet registrations and either inline SQL or a
query file. The embedded DuckDB currently needs the local core-functions and Parquet extension
paths explicitly:

```bash
cd experimental/datafusion
LD_LIBRARY_PATH="../../build/release/extension/sirius${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
SIRIUS_DUCKDB_CORE_FUNCTIONS_EXTENSION="../../build/release/extension/core_functions/core_functions.duckdb_extension" \
SIRIUS_DUCKDB_PARQUET_EXTENSION="../../build/release/extension/parquet/parquet.duckdb_extension" \
  cargo run --release -- \
  --table lineitem=/path/to/tpch/lineitem \
  --sql 'SELECT COUNT(*) FROM lineitem'
```

## Layout

- `src/lib.rs`: library boundary consumed by benchmark runners
- `src/main.rs`: standalone single-query CLI
- `src/engine.rs`: dedicated Sirius context thread
- `src/plan.rs`: SQL-to-Substrait conversion and table binding
- `src/compare.rs`: reserved for correctness-result comparison

## Benchmark integration

The libcudf benchmark runner should remain the owner of dataset preparation, query files, warmup,
iteration timing, JSON output, and reports. Sirius runs in a separate executable because the Sirius
Pixi environment and the libcudf benchmark wheel can use different libcudf ABIs. The harness still
passes the same dataset, query selection, warmup, partition count, and iteration count to all three
engines.

Build both runners, then launch the comparison harness:

```bash
pixi run cargo build --manifest-path experimental/datafusion/Cargo.toml --release --bin sirius-dfbench
cd experimental/datafusion/libcudf-rs
pixi run cargo build -p benchmarks --release
target/release/dfbench harness --dataset tpch_sf10 --query q1,q6 --iterations 5 --warmup
```

The harness runs DataFusion CPU, libcudf-datafusion, and Sirius in separate subprocesses and writes
compatible JSON results and a combined report under
`libcudf-datafusion-benchmarks/benchmark-results`. Use `--sirius-executable` to override the default
`experimental/datafusion/target/release/sirius-dfbench` path.

## Fresh checkout

The libcudf benchmark fork is pinned as a submodule. From the Sirius repository root:

```bash
git submodule update --init experimental/datafusion/libcudf-rs
repo_root=$PWD
pixi install
pixi run make
pixi run cargo build \
  --manifest-path experimental/datafusion/Cargo.toml \
  --release \
  --bin sirius-dfbench

cd experimental/datafusion/libcudf-rs
pixi run cargo build -p benchmarks --release
target/release/dfbench prepare-tpch \
  --sf 10 \
  --output libcudf-datafusion-benchmarks/data/tpch_sf10
pixi run env SIRIUS_BUILD_DIR="$repo_root/build/release" \
  target/release/dfbench harness \
  --dataset tpch_sf10 \
  --query q1,q6 \
  --iterations 5 \
  --warmup \
  --enable-cudf-parquet-scan
```

The generated dataset, benchmark results, Cargo targets, and downloaded cuDF libraries are ignored
and must be recreated on each machine. The native cuDF Parquet scan flag avoids a misaligned-address
failure observed in the default Arrow upload path on an RTX PRO 6000 Blackwell.
