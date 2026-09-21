# Sirius SQL correctness tests

This runner loads an existing Sirius extension, executes ordinary SQL with fallback disabled, and compares its results with a separate DuckDB CPU worker. It uses **sqllogictest-rs** for the file format and completion, **Arrow** for typed results, IPC, sorting and equality, **approx** for floating-point tolerances, and **sqlparser-rs** for benchmark ordering adaptations.

Passing means the output matches DuckDB. Existing artifacts do not expose reliable execution counters through SQL, so this runner does **not** claim verified GPU coverage. The C++ integration tests continue to provide those internal execution assertions.

## Build

The runner is a standalone package within the Rust workspace. It does not link `sirius-sys` or build Sirius/CUDA. Use a DuckDB shared library compatible with the artifact, preferably built from its exact DuckDB submodule revision. Distribution's version label is not sufficient to identify the patched source revision.

```bash
export DUCKDB_LIB_DIR=/absolute/path/to/runtime/lib
export DUCKDB_INCLUDE_DIR=/absolute/path/to/runtime/include
export LD_LIBRARY_PATH="$DUCKDB_LIB_DIR:${LD_LIBRARY_PATH:-}"
pixi run --manifest-path tools/sqltest/pixi.toml cargo build \
  --manifest-path rust/Cargo.toml -p sirius-sqltest --locked
```

`tools/sqltest/build-runtime.sh REVISION v1.5.5 OUTPUT` builds a compatible CPU-only shared runtime. It needs `gh` authentication for source downloads. Rust bindings are pinned to DuckDB 1.5.5; update the binding/runtime combination together when upgrading DuckDB.

Run the helper through the isolated environment:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml bash \
  tools/sqltest/build-runtime.sh DUCKDB_COMMIT v1.5.5 .cache/sqltest/runtime
```

Use the DuckDB submodule commit belonging to your extension build for `DUCKDB_COMMIT`. The helper produces `lib/libduckdb.so` and `include/duckdb.h` for the variables above. The isolated environment supplies Rust and the CPU build tools; it does not install a CUDA development environment.

Check the local setup with the self-contained regressions; no benchmark data is needed:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --cpu-only --suite regressions --output runs/cpu-check-001
```

## Prepare and run

Fixture preparation may download DuckDB generator extensions. Query execution does not download extensions or data. Source URLs, checksums, recipes, profiles, and named sweeps are in `sqltest.toml`. Suites are discovered under its `suite_roots`.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest prepare \
  --run benchmarks --source hits=/absolute/path/to/test_hits.tsv

pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --suite tpch --output runs/tpch-001

pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --run matrix --output runs/matrix-001

pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --select tpch/q01 --output runs/q01-001
```

The initial suites are `tpch`, `tpcds`, `clickbench`, and `regressions`; `--suite all` selects every discovered suite. `--select` also accepts a file substring or `suites/tpch/q01.slt:LINE`. Preceding setup and includes still execute; other query assertions are skipped. Tests must keep mutations in setup statements, not queries.

`--cpu-only` explicitly validates the harness and snapshots without Sirius or a GPU. Such reports are labelled separately and do not apply the GPU gap baseline. Two-GPU profiles never silently fall back to one GPU. Output directories must be new or empty.

Fixtures use TPC SF0.01 and the first 100,000 records of the pinned ClickBench TSV. Parquet files and their checksums are cached under `.cache/sqltest/data`. Re-running preparation verifies the cache; choose a new `--output`/`--fixtures` directory after changing the runtime or fixture recipe. Small datasets deliberately produce some empty results; reports count these separately.

Shared fixtures create `TABLE` or `VIEW` relations over the same Parquet data using `__RELATION__` and `__FIXTURE_ROOT__` substitutions. Native workers checkpoint an isolated disk database before query execution. The regression suite declares only native storage in the manifest, so matrix runs do not duplicate its self-contained tests.

## Configure runs

Each immediate subdirectory of a `suite_roots` directory becomes a suite if it contains `.slt` files (searched recursively) or a `suite.toml`. The directory name is the suite name. An optional `suite.toml` declares supported storage modes, fixture dependencies, and an import recipe. Without it, a suite needs no prepared fixtures and accepts all storage modes. For example:

```toml
# suites/my-regressions/suite.toml
storage = ["native"]
```

`sqltest.toml` defines named axes and their choices. Runs select choices from each axis; the runner expands their Cartesian product and filters out storage modes that a suite does not support. A single choice fixes an axis, a list sweeps selected choices, and `"all"` sweeps every declared choice. Adding suites, axes, choices, or runs does not require changing Rust.

```toml
[axes.optimizers.default]

[axes.optimizers.fixed-joins]
settings = { disabled_optimizers = "join_order,build_side_probe_side" }

[runs.smoke]
suites = ["regressions"]
axes = { gpu = ["one"], storage = ["native"], optimizers = ["default"] }

[runs.optimizer-sweep]
suites = "all"
axes = { gpu = ["one"], storage = ["native"], optimizers = "all" }
```

An axis choice can select a GPU `profile`, a `storage` mode, and SQL `settings`. Settings apply to the candidate worker; the CPU reference keeps its defaults unless the choice explicitly supplies `reference_settings`. An empty choice, such as `optimizers.default`, preserves the loaded engine's defaults. Setting names must be lowercase; values are TOML booleans, integers, finite floats, or strings. The runner owns GPU execution and fallback settings. Conflicting choices, unknown references, and malformed definitions fail before workers start.

Optimizer presets are explicit replacement lists. For example, `fixed-joins` disables exactly `join_order` and `build_side_probe_side`; it does not merge with Sirius's disabled optimizers. This can deliberately enable unsupported plans and reveal support gaps. Use `default` to retain the artifact's optimizer defaults.

The supplied runs are `smoke` (self-contained regressions), `local` (all suites, one GPU, native tables), `benchmarks` (three benchmark suites, one GPU, both storage modes), `matrix` (all suites and GPU/storage combinations with default optimizers), `optimizer-sweep`, and `full-sweep` (all suites and choices on all three axes). The default run is set by `default_run`. `--suite` overrides suite selection; repeatable `--axis NAME=CHOICE[,CHOICE]` or `--axis NAME=all` overrides or adds an axis.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run --run matrix --dry-run
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run matrix --axis gpu=one --axis optimizers=fixed-joins \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/fixed-joins-001
```

Fixture definitions list SQL recipe files, generator extensions, required input sources, and tables to export as Parquet. Source files are bound with repeatable `--source NAME=PATH` arguments and verified against the manifest's SHA-256 before execution. The recipe SQL controls scale and sampling; the runner has no benchmark-name dispatch. Recipe changes invalidate the corresponding cached fixture.

## Generated tests

An external directory of self-contained `.slt` files uses the same runner through `--suite-dir NAME=PATH`. It needs no central registration or `suite.toml`:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --suite-dir generated=/tmp/generated-sql-tests --suite generated \
  --axis optimizers=all --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/generated-001
```

Each generated file can create and populate its own tables, then compare query results with DuckDB. Give each query a stable ID, such as `generated/seed-17/query-1`, and preserve the seed in the file. This provides a replay path for a future fuzzer; generation and shrinking are outside the current scope.

## Write a regression

```text
statement ok
CREATE TABLE t(i INTEGER);

statement ok
INSERT INTO t VALUES (1), (1), (NULL);

# sirius: id = "regressions/example"
# sirius: tags = ["aggregate", "null", "duplicates"]
query I
SELECT count(i) FROM t;
----
```

Stable IDs are required. Metadata is TOML on `# sirius:` lines immediately preceding a query. Supported keys are `id`, `tags`, `snapshot`, `timeout` (seconds, default 120), and `tolerances` (zero-based columns). Includes are relative to their containing file, support globs, preserve source locations, and reject cycles.

Generate an expected-output snapshot from DuckDB:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest complete \
  test/sqltest/suites/regressions/example.slt
```

`complete` only uses CPU, adds `snapshot = true`, and rewrites the requested file after successful execution. It does not rewrite included files or gap expectations. Review the generated output before committing. Snapshots quote text using JSON escaping so whitespace, embedded separators, empty strings, and literal `"NULL"` remain distinguishable from SQL NULL.

Use `rowsort` for unordered multisets and `nosort` for deterministic ordered output. Duplicates count. `valuesort` is rejected because it discards row relationships. Negative tests use native `query error REGEX`/`statement error REGEX` syntax. Retry, shell-command, conditional, and multiple-connection SLT records are intentionally rejected rather than silently ignored.

Floating-point comparisons are exact unless a test opts into tolerances:

```text
# sirius: tolerances = { "1" = { absolute = 1e-8, relative = 1e-9 } }
```

Approximate comparisons require deterministic row order or unique exact key columns. Ambiguous unordered matching is a harness limitation; the runner does not guess pairings. Decimal values and large integers remain exact. Arrow schema and DuckDB logical types must also match.

## Reports and known gaps

Each run saves its resolved `plan.json`, discovered `suites.json`, and `sqltest.toml`, plus `report.json`, `summary.md`, `junit.xml`, and per-case SQL, settings, Arrow results, text output, and error details. `worker-logs.txt` points to the reference/Sirius process logs. A worker crash or timeout blocks the remainder of that file and leaves independent files runnable. Reports are saved after each case so interrupted runs retain partial results.

Start with `summary.md` for per-suite/configuration match counts and links to failures. A case's files live under `cases/CONFIGURATION_HASH/ID/`; `configuration.json` records its named axis choices and resolved settings. Compare `reference.txt` and `sirius.txt` for mismatches, or read `result.json` and the worker logs for execution failures. Re-run the case with `--select ID`, the same axis choices, and a fresh output directory.

`gaps.toml` is an explicit baseline, initially empty. Add a gap only after linking a reviewed issue. Infrastructure, reference, and harness failures cannot be accepted as engine gaps. An expected failure remains visible as its raw outcome; a changed failure or unexpected pass fails the baseline check.

```toml
[[gaps]]
id = "tpch/q01"
axes = { gpu = "one", storage = "native", optimizers = "default" }
outcome = "error"
issue = "https://github.com/sirius-db/sirius/issues/123"
error = "specific error pattern"
```

Gap coordinates must name every axis in the tested configuration; expectations do not automatically extend to new sweep choices. Remove the initial `gaps = []` when adding entries. Do not accept all observed failures automatically: crashes, harness defects, and incorrect results require inspection.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest report \
  runs/matrix-002/report.json --previous runs/matrix-001/report.json
```

History comparisons require a complete prior report and compatible runtime/device/comparison policy. Query, setup, fixture, and configuration changes are identified separately. Exit status is 0 for a passing baseline, 1 for failed/incomplete results, and 2 for invocation/provisioning failures.

## Corpus provenance

All 22 TPC-H, 99 TPC-DS, and 43 ClickBench IDs are imported. `upstream/` retains the original SQL. `provenance.json` records source versions. Ordered or limited benchmark queries append output-column tie breakers before LIMIT/OFFSET; the imported queries are correctness adaptations, not official benchmark submissions.

The maintenance `import --suite NAME` command reads each suite's import definition from its `suite.toml`: schema SQL, extension dependencies, query source, expected count, and provenance. Paths in these definitions are relative to the corpus root. The bundled definitions use `tpch_queries()` and `tpcds_queries()` from the pinned runtime and a pinned ClickBench SQL file. It regenerates benchmark files and fixtures; it is not run during normal tests.

Nightly artifact testing and a published history dashboard are follow-up work. Local JSON reports already support comparison through `--previous`.
