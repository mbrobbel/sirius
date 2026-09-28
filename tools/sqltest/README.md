# SQL test tools

Build the Rust runner once, then add and run SQLLogicTest files against an existing
Sirius extension. Adding SQL tests does not require rebuilding the C++ test binary.
The separate Pixi environment installs DuckDB's library and headers.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml cargo build \
  --manifest-path rust/Cargo.toml --locked -p sirius-sqltest
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --cpu-only --run smoke --output runs/smoke-cpu
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --extension /absolute/path/to/sirius.duckdb_extension --run smoke \
  --output runs/smoke-gpu
```

Use a fresh output directory for each run.

Suites are discovered under `test/sqltest/suites`. Each `.slt` file can create and
populate its own tables, include shared SQL fixtures, and compare query results
with DuckDB. No C++ registration is needed.

The SQL corpus initially supplements the existing C++ tests. C++ migration is
deferred until fuzz integration. The fuzzer can keep its own generation and replay
workflow; exporting findings as `.slt` cases is a later integration step. C++ tests
continue to verify internal execution counters and state.
