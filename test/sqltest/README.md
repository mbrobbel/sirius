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

## Specialized runs

`legacy-tpch` preserves the original extension test's SF1 table files, schema,
query variants, and complete CSV goldens. It is separate from the smaller
benchmark fixture. Generate its inputs from the repository's original dbgen
archive, prepare the checksum-verified cache, then run:

```bash
pixi run bash setup_test_datasets.sh --tpch-only
pixi run --manifest-path tools/sqltest/pixi.toml sqltest prepare --run legacy-tpch
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run legacy-tpch --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/legacy-tpch-001
```

The original CSV goldens live under `fixtures/goldens/tpch`; each file has one
copy, moved unchanged from the retired test harness. CPU setup reads them with explicit SQL
types and checks every row in order against DuckDB. The
runner then compares Sirius with that checked reference. This uses the same
fixture and statement machinery as other suites.

The runner supports transparent Sirius execution only. The `legacy-tpch` name
identifies the source of these fixtures, not an execution mode. The old legacy
SQL test files, unused goldens and generators, Python test script, and DuckDB
extension test registration are removed. Engine code and performance tools are
unchanged.

`compressed-gate` runs the compressed-materialization residency cases with
their original 2 GiB GPU memory cap and 16 KiB scan batches. These settings
preserve the multi-chunk pinning premise. `compressed-partition` uses the same
memory cap with 256 KiB scan batches, 64 KiB hash partitions, and a 1 KiB build
limit to exercise narrow carriers across partition exchanges. Both suites are
included in `correctness` with fixed profiles. Their named runs are convenience
selections for running either family alone:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run compressed-gate --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/compressed-gate-001
```

`hive-watchdog` similarly selects the four Hive watchdog cases, preserving
their 60-second timeout and original memory/thread settings. They also belong
to `correctness`.

`partition_memory` preserves the single-GPU partition-memory regression in
`correctness`: eight two-million-row Parquet files, a 200,000-row build table,
a 512 MiB GPU limit, and five consecutive joins. The fixed profile preserves
the original scan, build, and partition limits. To run its two-GPU variant:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run partition-memory-multi-gpu \
  --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/partition-memory-multi-gpu-001
```

These suites use the original fixture defaults; exploratory size or memory
changes belong in separate suites or configuration sweeps. Reproductions retain
each worker's Parquet files and profile.

`operators-multi-gpu` combines the two-GPU partition-memory case with the
sort, grouped-aggregate, hash-join, broadcast, and Q11-shaped operator cases.
Each suite has a fixed profile preserving its original memory and partition
settings. DuckDB supplies the reference even where the old helper accidentally
ran its reference query on the GPU. Those cases keep two consecutive queries
per original comparison; repeated build/probe and scale-up cases keep eight
and six GPU queries respectively. CPU-only validation works on machines
without two GPUs:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run operators-multi-gpu --cpu-only --output runs/operators-cpu-001
```

The original operator C++ comparisons retain coverage of their legacy explicit
entrypoint. C++ also checks per-device scheduling and broadcast/filter publication.
The old helper's `cache` and `duckdb_scan_num_threads` fields were never emitted
into its YAML; the SQL profiles preserve the configuration actually used.

`tpch-multi-gpu` runs the migrated TPC-H suites with the original two-GPU
integration profile. `sf10` runs the four original SF10 cases and requires two
GPUs plus eight supplied Parquet files. Its suite declares `minimum_gpus = 2`,
which excludes incompatible profiles from a sweep. Missing physical GPUs are
reported as unavailable infrastructure; a two-GPU run never falls back to one.
Local development on one GPU does not require running these selections. C++
retains their internal routing and dynamic-filter assertions; SQL owns result
comparisons. CI selects the original 44 native/Parquet TPC-H queries on two GPUs.

SF10 sources use `kind = "provided"`. Each requires a local `--source` binding
when first prepared. Their input hashes are recorded in the fixture manifest;
the cached files are checked on each run. Supplying changed inputs to an
existing cache fails; use a new preparation directory.

```bash
inputs=()
for table in customer lineitem nation orders part partsupp region supplier; do
  inputs+=(--source "sf10-${table}=/absolute/path/to/sf10/${table}.parquet")
done
pixi run --manifest-path tools/sqltest/pixi.toml sqltest prepare \
  --run sf10 "${inputs[@]}"
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run sf10 --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/sf10-001
```

Neither SF10 nor previously disabled cases are part of `correctness`. The latter
include 19 restored CSV cases: they pass CPU validation and currently report
Sirius's unsupported `read_csv` error with fallback disabled.

Two opt-in double-inequality cases keep Sirius's original FULL JOIN but use
an equivalent INNER JOIN for the DuckDB oracle. Their WHERE predicates reject
all unmatched rows, so row multiplicities, ordering, and LIMIT are unchanged.
Engine-specific temporary views express this in SQLLogicTest without runner
special cases. The original data scale and fallback policy are preserved.

### Managed S3 service

The `s3` run starts a private MinIO container for each SQL script through
Testcontainers, uploads checksum-verified fixtures, and removes the container
after its workers exit. It requires a reachable Docker daemon; `DOCKER_HOST`
can select one. The Docker CLI is not required. CPU-only runs, completion,
preparation, and dry runs do not start containers.
The runner pulls `quay.io/minio/minio` using the manifest's explicit `image_tag`;
the fully qualified image is recorded with the service results.
The supplied S3 services mount `/data` on a private, 2 GiB tmpfs. This limit
is a ceiling, not a reservation; the SF1 objects occupy about 311 MiB. Object
data is discarded with the container, and uploads do not depend on the free
space in Docker's data directory. Docker still needs disk space for images and
logs. Set `storage = { kind = "container" }` to use its writable layer instead.
The storage policy is recorded in service metadata and comparison fingerprints.

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest prepare --run s3
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run s3 --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/s3-001
```

When running on the host with artifacts built inside a container, use:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest-host \
  --extension /absolute/path/to/sirius.duckdb_extension \
  s3 s3-sf1 s3-pagination
```

The Rust launcher uses the already-built runner and the activated environment's
`DUCKDB_LIB_DIR` (override with `--duckdb-lib-dir PATH`) and creates a new
`runs/host-*` directory. It finds GPU libraries in this worktree's Pixi environment or in the
host source of a running container's mount at this worktree's `.pixi` path.
Use `--gpu-lib-dir PATH` to select the directory explicitly. Docker mount
discovery requires a local daemon and a readable source directory. On NixOS,
the launcher also adds `/run/opengl-driver/lib` when it exists to resolve NVIDIA
driver libraries, and preserves the inherited `LD_LIBRARY_PATH`. Missing shared
libraries fail before fixture preparation or MinIO startup. Named runs
execute sequentially, and failures do not prevent the remaining runs from
producing their reports.

Declare services in `sqltest.toml` and select one with
`object_store = "local-s3"` in the suite's manifest:

```toml
[services.local-s3]
kind = "minio"
image_tag = "RELEASE.2025-09-07T16-13-09Z-cpuv1"
bucket = "sirius-test"
storage = { kind = "tmpfs", size_mib = 2048 }

[services.local-s3.objects]
"parquet/nation.parquet" = { fixture = "tpch-parquet-original", path = "nation.parquet" }
```

Object keys are literal, including `%`, `?`, and `#`; their fixture paths must
name declared outputs. Service fixtures are automatically included in preparation
and verification. The managed service uses the C++ fixture's test credentials
(`minioadmin` / `minioadmin`) and region (`us-east-1`). Its endpoint and credentials
are bound into a private copy of the selected Sirius YAML profile.

Use `onlyif duckdb` setup to create local reference views and `onlyif sirius`
setup for S3 views. `__S3_BUCKET__` expands in Sirius SQL. S3 view binding requires
GPU execution, so its setup statement explicitly enables `gpu_execution` before
creating the views; query execution still enforces the normal fallback policy.
Each setup statement starts with GPU execution disabled. Put `SET gpu_execution
= true;` in the same `statement ok` block as the S3 `CREATE VIEW`; a separate
statement does not carry that setting into the next setup operation.

For repeated objects, declare a counted copy recipe:

```toml
[[services.s3-glob-scale.copies]]
source = { fixture = "tpch-parquet-original", path = "nation.parquet" }
key_prefix = "glob-scale/part_"
key_suffix = ".parquet"
count = 1001
```

This expands keys from `part_0.parquet` through `part_1000.parquet`, preserving
literal prefix and suffix bytes. Counts must be positive; generated keys must
be valid and distinct from all other declared keys. Each worker also receives
the service's complete object layout under `__TEST_DIR__/.sqltest-objects`.
A DuckDB substitution can point there to compare the same glob with local files.
These private copies are rebuilt during completion and reproduction, without
Docker. Source fixtures retain their hashes; service recipes and uploaded object
hashes are recorded in results.

The explicit `s3-pagination` run preserves the original 1,001-object LIST-page
regression and its expected count, sum, minimum, and maximum. Prepare it with
`sqltest prepare --run s3-pagination`, then use `sqltest run --run s3-pagination`
with the extension artifact.

Each case records the service recipe, runtime endpoint, container ID, uploaded
object hashes, and both original and resolved profiles. Service recipes affect
comparison fingerprints; random ports do not. Replaying the saved `repro.slt`
and `suite.toml` through `--suite-dir` starts a fresh service using the original
corpus manifest. The resolved `repro.sql` refers to the previous container and
is for inspection. MinIO logs are saved beside the worker logs when available.
Startup or upload failures produce infrastructure failures and an incomplete
report, rather than successful skips.

Suites can declare different literal contents for DuckDB and Sirius:

```toml
[substitutions.OBJECT_ROOT]
duckdb = "__FIXTURE_ROOT__/s3-surface-original"
sirius = "s3://__S3_BUCKET__"
```

```sql
SELECT count(*)
FROM read_parquet('__OBJECT_ROOT__/parquet/nation.parquet');
```

Both executed queries retain a literal path, which Sirius needs to recognize S3
scans. Names use uppercase letters, digits and single underscores; built-in names
are reserved. Declare both engine values. Values are unescaped string contents,
not SQL expressions: the runner escapes single quotes once. They can reference
`__FIXTURE_ROOT__`, `__TEST_DIR__`, and `__S3_BUCKET__` (which requires an
`object_store`). Custom substitutions cannot reference one another. Unknown
placeholders fail before execution. The `__NAME__` syntax is reserved in SQL.
`__RELATION__` remains a built-in SQL keyword substitution.

CPU-only runs use DuckDB values for both workers, and `complete` uses DuckDB
values. Saved suite manifests retain substitutions for replay, and changes to
either engine's values change comparison fingerprints.

The initial `s3_tpch` suite preserves the original tiny Q1–Q22 sequence, Parquet
files, memory profile, and floating-point tolerance. It is separate from
`correctness`. All 57 regular S3 cases, 22 SF1 cases, and the pagination case
have passed against DuckDB with managed MinIO and fallback disabled. Surface
cases use literal path substitutions and preserve encoded-key layouts and fixed
profiles. C++ retains execution counters, REST routing and cache checks, raw
object-list assertions, and negative-error checks; SQL owns result comparisons.
Legacy `gpu_execution('…')` and `gpu_processing` entrypoints are outside this
migration; the runner uses transparent SQL execution.

The `s3-sf1` run preserves the transparent SF1 Q1–Q22 sequence separately from
the tiny S3 run:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest prepare --run s3-sf1
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run s3-sf1 --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/s3-sf1-001
```

Its fixture uses the original `dbgen(sf=1)` recipe and Snappy Parquet encoding.
SQL fixture recipes accept `compression = "snappy"` or `"zstd"`; the default is
Zstd. The fixed SF1 profile retains the original 2 GiB GPU, 4 GiB host, and
16 GiB disk capacities. CPU-only validation uses local Parquet files without
starting MinIO. The retained C++ TPC-H tests verify GPU routing at both scales.

## Generated tests

An external directory of self-contained `.slt` files uses the same runner through `--suite-dir NAME=PATH`. It needs no central registration or `suite.toml`:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --suite-dir generated=/tmp/generated-sql-tests --suite generated \
  --axis optimizers=all --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/generated-001
```

Each generated file can create and populate its own tables, then compare query results with DuckDB. Give each query a stable ID, such as `generated/seed-17/query-1`, and preserve the seed in the file. This provides a replay path for a future fuzzer; generation and shrinking are outside the current scope.

Use `__TEST_DIR__` for files created by a script, such as
`COPY t TO '__TEST_DIR__/t.parquet' (FORMAT PARQUET)`. It expands to a separate
scratch directory for each worker, preventing the reference and candidate from
overwriting one another's fixtures. Reproduction SQL records the resolved paths.

For nested layouts, declare `scratch_directories = ["parts", "hive/part=2024"]`
in `suite.toml`. Each worker creates these relative paths below its own
`__TEST_DIR__`. Parent traversal and absolute paths are rejected. Saved
reproductions retain the declarations, and `complete` uses them too.

For fixtures whose metadata contains relative file references, use a
`fixture_files` map in `suite.toml`:

```toml
fixture_files = { "test/cpp/integration/data" = "iceberg-original" }
```

The map names prepared fixtures to copy into each worker's private directory.
It also declares those fixture dependencies; no duplicate `fixtures` entry is
needed. Paths must be relative and destinations must not overlap. Every source
file is verified before copying. Workers can modify their own copies without
changing the cache or another worker's inputs. Reproductions and `complete`
retain the same layout.

File fixtures can declare `extensions = ["avro", "iceberg"]` to provision
reader dependencies during `prepare`, including when reusing cached data.
Test scripts only `LOAD` extensions; they do not install them. The Iceberg
suite preserves 174 original data and metadata files, literal expected rows,
delete-file premises, snapshot IDs, and explicit fallback policies.

The `correctness` run includes migrated integration cases and selects both
native and Parquet storage. Each suite declares its supported storage modes:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --run correctness --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/correctness-001
```

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

Stable IDs are required. Metadata is TOML on `# sirius:` lines immediately preceding a query. Supported keys are `id`, `tags`, `snapshot`, `timeout` (seconds, default 120), `execution`, and `tolerances` (zero-based columns). Includes are relative to their containing file, support globs, preserve source locations, and reject cycles.

`float_tolerance = { absolute = 0.0001, relative = 0.0 }` supplies a default for
FLOAT/DOUBLE columns when supplied datasets may use different numeric types.
Decimal and integer columns remain exact. Per-column `tolerances` override that
default and must still name floating columns. The same deterministic alignment
requirements apply: ordered results or unique exact key columns.

Generate an expected-output snapshot from DuckDB:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest complete \
  test/sqltest/suites/regressions/example.slt
```

`complete` only uses CPU, adds `snapshot = true`, and rewrites the requested file after successful execution. It does not rewrite included files or gap expectations. Review the generated output before committing. Snapshots quote text using JSON escaping so whitespace, embedded separators, empty strings, and literal `"NULL"` remain distinguishable from SQL NULL.

Use `rowsort` for unordered multisets and `nosort` for deterministic ordered output. Duplicates count. `valuesort` is rejected because it discards row relationships. Negative tests use native `query error REGEX`/`statement error REGEX` syntax. Retry, shell-command, and query conditions are intentionally rejected rather than silently ignored. Setup statements support the engine conditions described below.

Floating-point comparisons are exact unless a test opts into tolerances:

```text
# sirius: tolerances = { "1" = { absolute = 1e-8, relative = 1e-9 } }
```

Approximate comparisons require deterministic row order or unique exact key columns. Ambiguous unordered matching is a harness limitation; the runner does not guess pairings. Decimal values and large integers remain exact. Arrow schema and DuckDB logical types must also match.

## Intentional fallback tests

Queries disable CPU fallback by default (`execution = "no_fallback"`). Tests
whose purpose is to exercise fallback can opt in explicitly:

```text
# sirius: id = "regressions/fallback_case"
# sirius: execution = "allow_fallback"
query I
SELECT count(DISTINCT value) FROM values_table;
----
```

The policy applies to that query only and is reset before the next query. Both
engines must still return matching results or the expected error. Reports group
matches by execution policy: a match with fallback allowed is a correctness
result, not evidence of GPU support. C++ tests retain counters that verify
whether fallback happened at plan time or runtime. Reproduction SQL includes
the execution settings and checkpoints used by the workers.

## Engine-specific setup

Use SQLLogicTest conditions on setup statements for settings or fixtures that
apply to one engine:

```text
onlyif sirius
statement ok
SET max_sort_partition_bytes = 65536;
```

`onlyif duckdb`, `skipif duckdb`, and `skipif sirius` are also supported.
Unknown labels, conditions on includes, dangling conditions, and conditions excluding both engines are errors. Query
assertions must run on both engines. CPU-only validation treats both workers as
DuckDB, and snapshot completion runs only DuckDB setup.

## Transactions and named connections

Use native SQLLogicTest `connection NAME` before each statement or query that
should use a named connection. The directive applies to the next record only;
other records use the default connection. Connections share the worker's
database and retain their transaction and session state throughout the file.
Commands execute sequentially, so these tests cover interleaved transactions;
concurrent execution and lock timing remain C++ tests.

Transaction suites must own checkpoint timing in `suite.toml`:

```toml
storage = ["native"]
checkpoint = "explicit"
```

This disables the runner's automatic checkpoint before each query. Place SQL
`CHECKPOINT` statements where the test needs them, such as before pinning a
table. `complete` uses the suite's checkpoint policy too.

```text
connection reader
statement ok
BEGIN TRANSACTION;

# sirius: id = "mvcc/reader_snapshot"
connection reader
query I
SELECT count(*) FROM t;
----

connection reader
statement ok
ROLLBACK;
```

Each case saves `repro.slt` with includes expanded and a `suite.toml` preserving
checkpoint policy and fixture dependencies. Replay through the runner to retain
connections, expected errors, and execution policies:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqltest run \
  --suite-dir replay=/absolute/path/to/case/artifacts --suite replay \
  --select mvcc/reader_snapshot --axis gpu=one --axis storage=native \
  --axis optimizers=default --extension /absolute/path/to/sirius.duckdb_extension \
  --output runs/replay-001
```

Use the original corpus root, prepared fixtures, and axis choices recorded in
`configuration.json`. Single-connection cases also save `repro.sql` and
`reference-repro.sql` with the applicable setup and preceding queries.

## SQL formatting

Pre-commit formats `.slt` files and fixture `.sql` files using the same DuckDB-aware formatter as the runner. The standalone formatter builds in the isolated CPU environment and needs no DuckDB runtime or Sirius artifact:

```bash
pixi run --manifest-path tools/sqltest/pixi.toml sqlformat
pixi run --manifest-path tools/sqltest/pixi.toml sqlformat --check
```

Pass individual files or directories to limit the selection. SQLLogicTest metadata and expected output remain intact. SQL comments, relation placeholders, negative syntax tests, and DuckDB statements the parser does not recognize are preserved verbatim. Generic spelling, quote, whitespace, and line-ending fixers exclude SQL test data so they cannot rewrite literals or expected results. Vendored SQL under `upstream/` is excluded from the hook.

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
