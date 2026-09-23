# Testing Sirius

Use the [SQL correctness runner](sqltest/README.md) for SQL regression tests and
DuckDB-versus-Sirius comparisons. Suites under `sqltest/suites/` are discovered
automatically; fixtures and configuration sweeps are declared in TOML. The runner
accepts a built `.duckdb_extension` and saves results, logs, and reproduction SQL.

Run the C++ unit and integration tests with:

```bash
pixi run make test
pixi run make test_debug
```

The [migration inventory](sqltest/MIGRATION.md) tracks which existing tests have
SQL replacements and which assertions still need to be preserved. The old SQL harness and standalone correctness runners are removed; performance
and profiling tools remain separate.
