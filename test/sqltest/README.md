# Sirius SQL correctness tests

This runner loads an existing Sirius extension, executes ordinary SQL with fallback disabled by default, and compares its results with a separate DuckDB CPU worker. It uses **sqllogictest-rs** for the file format and completion, **Arrow** for typed results, IPC, sorting and equality, **approx** for floating-point tolerances, and **sqlparser-rs** for benchmark ordering adaptations.

Passing means the output matches DuckDB. Existing artifacts do not expose reliable execution counters through SQL, so this runner does **not** claim verified GPU coverage. The C++ integration tests continue to provide those internal execution assertions.

## Build

The runner is a standalone package within the Rust workspace. It does not link `sirius-sys` or build Sirius/CUDA. Its Pixi environment installs conda-forge's pinned `libduckdb` and `libduckdb-devel` packages and configures the library and header paths.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml cargo build \
  --manifest-path rust/Cargo.toml -p sirius-sqltest --locked
```

This builds both `sirius-sqltest` and the Rust `sirius-sqltest-host` launcher.
The launcher does not link DuckDB, so it can locate shared libraries before
starting the runner.

Rust bindings and the packaged runtime are pinned to DuckDB 1.5.5; update and test them together when upgrading DuckDB. Reports record the loaded library's SHA-256 and DuckDB version. A packaged runtime has no Sirius fork revision; leave `--runtime-revision` unset unless using a source build with a known revision. The isolated environment supplies Rust and CPU build tools, without a CUDA development environment.

Fixture caches are bound to the runtime hash. When changing runtimes, prepare into a fresh `--output` directory and pass it as `--fixtures` when running, or move aside `.cache/sqltest/data` before preparing again.

Check the local setup with the self-contained regressions; no benchmark data is needed:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --cpu-only --suite regressions --output runs/cpu-check-001
```

## Prepare and run

Fixture preparation may download DuckDB generator extensions. `prepare --download` also fetches missing input sources declared in the manifest, checks their decompressed SHA-256, and caches them. Query execution does not download extensions or data. Source locations, checksums, recipes, profiles, and named sweeps are in `sqltest.toml`. Suites are discovered under its `suite_roots`.

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

The initial suites are `tpch`, `tpcds`, `clickbench`, and `regressions`; `--suite all` selects every discovered suite. `--select` accepts an exact case ID, a file substring, or `suites/tpch/q01.slt:LINE`; exact IDs take precedence over file matches. Preceding statements and queries execute in order through the last selected query. Earlier queries are checked as prerequisites, with artifacts under `prerequisites/`; a failed prerequisite blocks the selected case. Only selected queries count in the report. Reproductions include the preceding query sequence.

`--cpu-only` explicitly validates the harness and snapshots without Sirius or a GPU. Such reports are labelled separately and do not apply the GPU gap baseline. Two-GPU profiles never silently fall back to one GPU. Output directories must be new or empty.

Fixtures use TPC SF0.01 and the first 100,000 records of the pinned ClickBench TSV. Parquet files and their checksums are cached under `.cache/sqltest/data`. Re-running preparation verifies the cache; choose a new `--output`/`--fixtures` directory after changing the runtime or fixture recipe. Small datasets deliberately produce some empty results; reports count these separately.

Shared fixtures create `TABLE` or `VIEW` relations over the same Parquet data using `__RELATION__` and `__FIXTURE_ROOT__` substitutions. Workers checkpoint their isolated disk database before each query by default; transaction suites can request explicit checkpoints. The regression suite declares only native storage in the manifest, so matrix runs do not duplicate its self-contained tests.

## Configure runs

Each immediate subdirectory of a `suite_roots` directory becomes a suite if it contains `.slt` files (searched recursively) or a `suite.toml`. The directory name is the suite name. An optional `suite.toml` declares supported storage modes, fixture dependencies, and an import recipe. Without it, a suite needs no prepared fixtures and accepts all storage modes. For example:

```toml
# suites/my-regressions/suite.toml
storage = ["native"]
```

Correctness runs choose a default profile and use any fixed `profile` declared
in a suite's `suite.toml`. Each suite runs once per supported storage mode;
there is no configuration sweep. For example:

```toml
[runs.correctness]
suites = "all"
profile = "integration-one-gpu"
```

A suite that needs small scan batches can declare
`profile = "compressed-materialization-gate"`. The profile is part of that
regression's setup. Correctness runs reject `--axis`; use a sweep run to explore
other configurations.

The supplied correctness selection excludes the separately named benchmark,
previously disabled, and SF10 runs. New regression suites are discovered
automatically without adding their names to this selection.

`sqltest.toml` also defines named axes and their choices for exploratory runs.
Sweep runs select choices from each axis; the runner expands their Cartesian product and filters out storage modes that a suite does not support. A single choice fixes an axis, a list sweeps selected choices, and `"all"` sweeps every declared choice. Adding suites, axes, choices, or runs does not require changing Rust.

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

An axis choice can select a GPU `profile`, a `storage` mode, and SQL `settings`. Settings initialize every connection in the candidate worker; the CPU reference keeps its defaults unless the choice explicitly supplies `reference_settings`. An empty choice, such as `optimizers.default`, preserves the loaded engine's defaults. Setting names must be lowercase; values are TOML booleans, integers, finite floats, or strings. The runner owns GPU execution and fallback settings. Conflicting choices, unknown references, and malformed definitions fail before workers start.

Optimizer presets are explicit replacement lists. For example, `fixed-joins` disables exactly `join_order` and `build_side_probe_side`; it does not merge with Sirius's disabled optimizers. This can deliberately enable unsupported plans and reveal support gaps. Use `default` to retain the artifact's optimizer defaults.

The supplied runs are `smoke` (self-contained regressions), `local` (all suites, one GPU, native tables), `benchmarks` (three benchmark suites, one GPU, both storage modes), `matrix` (all suites and GPU/storage combinations with default optimizers), `optimizer-sweep`, and `full-sweep` (all suites and choices on all three axes). The default run is set by `default_run`. `--suite` overrides suite selection; for sweep runs, repeatable `--axis NAME=CHOICE[,CHOICE]` or `--axis NAME=all` overrides or adds an axis. Sweep runs use their selected profiles independently of suite correctness profiles.

Runs can combine discovery with `exclude_suites = ["tpch_sf10"]`.
`local`, `matrix`, and `optimizer-sweep` exclude specialized SF10 and previously
disabled suites. Explicit `--suite NAME` or `--suite all` overrides exclusions.
`previously-disabled` preserves the original opt-in TPC-H and Hive tests; some have very
expensive CPU plans and may hit the query timeout. `full-sweep` explicitly
includes these suites.

GPU profiles can declare an `environment` map applied before the candidate
worker starts. Integration profiles preserve the original C++ memory pools and
operator batch limits and declare `SIRIUS_ENABLE_TEST_OPTIONS = "1"` to register
internal test settings. The lighter profiles remain available for smoke tests
and exploratory sweeps. Profile environment values are recorded in
`environment.json`, included in comparison fingerprints, and stored in the run
manifest; use them for non-secret configuration. `SIRIUS_CONFIG_FILE` and
`SIRIUS_DISABLE` are controlled by the runner.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run --run matrix --dry-run
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run matrix --axis gpu=one --axis optimizers=fixed-joins \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/fixed-joins-001
```

Fixture definitions list SQL recipe files, generator extensions, required input sources, and tables to export as Parquet. A source is either a `file` with a path relative to the corpus root, or a `download` with a URL and explicit `none`/`gzip` compression. Sources can also be bound with repeatable `--source NAME=PATH` arguments. Every input is verified against the manifest's SHA-256 before execution. The recipe SQL controls scale and sampling; the runner has no benchmark-name dispatch. Recipe changes invalidate the corresponding cached fixture.

Prefer self-contained SQL setup or shared SQL recipes for new regression data:
`CREATE TABLE`, `INSERT`, `range`, and deterministic expressions cover most cases.
Use the TPC generator extensions for benchmark data, and keep generated databases
and Parquet files in the ignored fixture cache. Reuse an existing input instead
of checking in another copy. Retain file fixtures when their physical encoding,
metadata, or malformed bytes are the regression condition. Small expected results
can live in the `.slt` file; broad comparisons use the live DuckDB oracle.

A fixture can instead copy original files without changing their storage
encodings:

```toml
[fixtures.original]
files = { "database.duckdb" = "original-database" }
```

Keys are relative output paths; values name input sources. Preparation copies
the files and records output hashes. Every run verifies the copies. The migrated
TPC-H suites use the original native database and Parquet files this way.
