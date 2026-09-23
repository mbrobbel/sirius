# SQL test migration

`migration.json` maps each ported query to its original file, test name, and line.
The source revision recorded there retains files removed after migration.
Query counts describe this corpus, not SQL feature completeness.

This migration targets transparent Sirius execution only. Legacy engine code
and C++ internals remain unchanged; the old legacy SQL test infrastructure is
removed. Performance tooling remains outside this correctness migration.
Some transparent suites reuse historical SQL and fixtures; this does not add
legacy execution support to the runner. Two-GPU runs are declared for suitable
CI runners; executing them on the single-GPU development host is not required.

| Original tests | SQL suite | Queries | Status |
| --- | --- | ---: | --- |
| `test_gpu_execution_aggregate_nulls.cpp` | `aggregate_nulls` | 11 | SQL comparisons; C++ execution assertions |
| `test_gpu_execution_allnull_scan.cpp` | `allnull_scan` | 15 | SQL comparisons; C++ execution assertions |
| `test_gpu_execution_filter_nulls.cpp` | `filter_nulls` | 62 | SQL comparisons; C++ execution assertions |
| `test_gpu_execution_join_nulls.cpp` | `join_nulls` | 9 | SQL comparisons; C++ execution assertions |
| `test_gpu_execution_unique_join.cpp` | `unique_join` | 7 | SQL comparisons; C++ execution assertions |
| `test_gpu_execution_tpcds_nulls.cpp` | `tpcds_nulls` | 54 | SQL comparisons; C++ execution and fixture assertions |
| `test_gpu_execution_order_nulls.cpp` | `order_nulls` | 12 | SQL positional comparisons; C++ execution assertions |
| `test_gpu_execution_null_safe_join.cpp` | `null_safe_join` | 29 | SQL comparisons; C++ execution and plan-fallback assertions |
| `test_gpu_execution_semantic_cast_fallback.cpp` | `semantic_cast` | 12 | SQL results and expected errors; C++ fallback and rejection assertions |
| `test_gpu_execution_array.cpp` | `array` | 38 | SQL comparisons and codec premises; C++ execution and storage assertions |
| `test_pin_table_type_drift.cpp` | `pin_type_drift` | 28 | SQL comparisons; C++ pin narrowing, cache serving, and execution assertions |
| `test_gpu_execution_parquet_nulls.cpp` | `parquet_nulls` | 84 | SQL comparisons and 90 fixture premises; C++ execution and storage assertions |
| `test_parquet_null_predicate_pushdown.cpp` | `parquet_pushdown` | 28 | SQL comparisons and fixture premises; C++ execution assertions |
| `test_pin_table_mvcc_delete.cpp` | `pin_mvcc_delete` | 28 | SQL comparisons; C++ execution and fallback assertions |
| `test_pin_table_mvcc_foundation.cpp` | `pin_mvcc_foundation` | 1 | SQL comparison; C++ execution, metadata, and WAL assertions |
| `test_pin_table_mvcc_insert.cpp` | `pin_mvcc_insert` | 40 | SQL comparisons; C++ execution, fallback, compression, and residency assertions |
| `test_pin_table_mvcc_update.cpp` | `pin_mvcc_update` | 1 | SQL comparison; C++ update guards, prepared statements, and concurrent checkpoint assertions |
| `test_gpu_execution_dense_count_join.cpp` | `dense_count_join` | 36 | SQL comparisons validated on both engines; C++ execution and setting assertions |
| `test_gpu_execution_dynamic_filter_native.cpp` | `dynamic_filter_native` | 10 | SQL comparisons validated on both engines; C++ execution and filter assertions |
| `test_gpu_execution_dynamic_filter_sip.cpp` | `dynamic_filter_sip` | 26 | Filter-off/on pairs validated on both engines; C++ publication and fixture assertions |
| `test_gpu_execution_tpch.cpp` | `tpch_integration_native`, `tpch_integration_parquet` | 447 | Original native and Parquet fixtures validated on both engines; C++ execution assertions |
| `test_gpu_execution_tpch.cpp` | `tpch_empty` | 6 | SQL results and schemas validated on both engines; C++ watchdog and execution assertions |
| `test_gpu_execution_tpch.cpp` | `tpch_disabled_native`, `tpch_disabled_parquet` | 16 | 14 CPU/GPU matches; two equivalent CPU oracles expose Sirius OOM errors; C++ retains internal assertions |
| `test_gpu_execution_tpch.cpp` | `tpch_sf10` | 4 | Supplied SF10 inputs CPU-validated; SQL comparisons and retained C++ route assertions; GPU validation deferred to a two-GPU host |
| `test_gpu_execution_cast_date_predicates.cpp` | `cast_date` | 277 | SQL comparisons validated on both engines; C++ execution and compression assertions |
| `test_gpu_execution_multi_format.cpp` (count carriers) | `parquet_count_carrier` | 10 | SQL comparisons validated on both engines; original nested layouts and C++ route assertions |
| `test_compressed_materialization_gate.cpp` | `compressed_materialization_gate` | 32 | Both engines validated with original memory/batch limits; C++ residency and narrowing assertions |
| `test_compressed_materialization_partition.cpp` | `compressed_materialization_partition` | 11 | Both engines validated; C++ exchange counters and physical-plan assertions |
| `test_gpu_execution_multi_format.cpp` (Hive layouts) | `hive_escaped`, `hive_disabled`, `hive_watchdog` | 17 | Both engines validated; C++ execution routes and watchdog retained |
| `test_gpu_execution_multi_format.cpp` (Iceberg) | `iceberg` | 36 | Both engines validated with original goldens; C++ routes, delete census, and CPU-reader guard retained |
| `test_pin_table_column_order.cpp` | `pin_column_order` | 2 | Both pin tiers validated; result-only C++ file removed |
| `test_pin_table_merge_columns.cpp` | `pin_column_merge` | 1 | SQL result validated; C++ merged cache metadata and mismatch assertions retained |
| `test_pin_table_host_streaming.cpp` | `pin_host_streaming` | 2 | Original 40-million-row fixture and 256 MiB GPU limit; C++ peak-memory assertion retained |
| `test_pin_table_zone_map_pruning.cpp` | `pin_zone_map`, `pin_zone_map_native` | 26 | Both engines validated; C++ chunk census and pruning probes retained |
| `test_transparent_execution.cpp` | `transparent` | 5 | SQL comparisons validated; C++ GPU routing and CPU bypass assertions retained |
| `test_transparent_runtime_fallback.cpp` (local nested fixtures) | `fallback_mix` | 29 | SQL comparisons and original goldens; C++ fallback routes and subprocess watchdogs retained |
| `test_transparent_runtime_fallback.cpp` (injected failures) | `runtime_fallback` | 7 | SQL results and transaction visibility; C++ runtime-fallback counters retained |
| `test_gpu_execution_vector_search.cpp` | `vector_search` | 25 | Exact DuckDB oracle versus ANN/ENN; C++ index state, chunk layout, and rejection checks retained |
| `test_partition_memspace_mgpu.cpp` | `partition_memory`, `partition_memory_mgpu` | 10 | Transparent single-GPU control validated on both engines; two-GPU variant CPU-validated; original legacy-entrypoint C++ tests retained |
| `test_physical_order_mgpu.cpp`, `test_physical_grouped_aggregate_merge_mgpu.cpp`, `test_physical_hash_join_mgpu.cpp` | `operator_mgpu_*` | 40 | CPU-validated; original configurations, scale, and consecutive executions preserved; legacy-entrypoint C++ checks retained |
| `test_gpu_execution_multi_format.cpp` (commented CSV) | `csv_disabled` | 19 | CPU validated; GPU correctly exposes unsupported CSV scans |
| `test_s3_tpch.cpp` (tiny) | `s3_tpch` | 22 | CPU/GPU comparisons validated with managed MinIO; C++ routing assertions retained |
| `test_s3_tpch.cpp` (SF1) | `s3_tpch_sf1` | 22 | Original SF1 generator, encoding, and profile; CPU/GPU comparisons validated; C++ routing assertions retained |
| `test_s3_sql_surface.cpp` (transparent surface) | `s3_surface*` | 35 | CPU/GPU comparisons validated; C++ retains REST/cache probes, raw LIST assertions, routing, and error checks |
| `test_s3_sql_surface.cpp` (pagination) | `s3_glob_scale` | 1 | Original 1,001 objects and goldens; CPU/GPU comparison validated; duplicate C++ case and upload recipe removed |
| `test/sql/tpch-sirius.test` | `tpch_legacy` | 22 | Transparent SQL coverage validated; old runner removed; goldens moved unchanged into this corpus |
| `test/sql/bugfix.test` | `legacy_regressions/issue_56` | 16 | Transparent SQL coverage with original expected results; old legacy test removed |
| `test/test_null.sql` | `legacy_regressions/nulls` | 11 | DuckDB snapshots added; old manual script removed |

The migrated C++ cases retain execution counters and fallback assertions through
`require_gpu_execution` and the existing fallback helpers. The SQL suites own
result comparisons. Intentional fallback cases explicitly set
`execution = "allow_fallback"`; all other queries disable fallback. The runner
resets this policy before every query and reports the two policies separately.
C++ counters still distinguish plan-time rejection from runtime fallback.

The TPC-DS NULL suite uses checksum-verified data from the original committed
DuckDB database, exported to Parquet and loaded into native tables. Eleven
fixture assertions require NULLs and unmatched join rows to exist. Decimal SUM
results are compared exactly; floating-point AVG retains the original `1e-6`
tolerance. ORDER BY cases preserve all four direction/NULL-position combinations
and the 64 KiB sort partition setting in Sirius-only setup.

Array cases preserve the original 3,000/5,000/200,000-row fixtures, query order,
NULL layers, signed and unsigned widths, and empty-input controls. SQL setup
requires Constant, RLE, BitPacking, and ALP/ALPRD segments where the original
tests required them. Large outputs use typed comparisons against DuckDB without
storing full snapshots in the repository.

Type-drift cases preserve GPU and host pin tiers, narrowing enabled/disabled at
query time, pin/drop/recreate sequences, and no-drift controls. C++ retains the
internal assertions that distinguish a cache hit from reading fresh disk data.

Parquet NULL cases preserve six fixture recipes, physical and logical types,
dictionary encoding, NULL populations, exact row-group sizes, and per-group NULL
counts. The 8,395-row fixture retains four 2,048-row groups and its 203-row tail;
ordered comparisons cover every group boundary and both 8,000-row run layouts.
Each worker generates its own files in an isolated scratch directory.

Parquet pushdown cases retain the 6,000-row flat and Hive-partitioned datasets,
NULL predicates, join predicates, and query order. Five SQL fixture assertions
verify NULL populations, exact flat row-group sizes, and the four 1,500-row Hive
partitions.

MVCC suites own checkpoint timing through `checkpoint = "explicit"` and use
named SQLLogicTest connections for older reader snapshots. Delete cases retain
tombstones, rollback, host and GPU pins, partial pins, and the original
13-million-row multi-chunk fixture with its 100 MB scan limit. Insert cases retain
transient and persistent deltas, constant and all-NULL segments, fixed-size
arrays, snapshot visibility, and the compression plan for the residency guard.
The update case checks pin rejection before checkpoint and the resulting value
after checkpoint. C++ keeps the concurrent execution and lock-timing tests.

Dense count-join cases preserve fused/unfused query pairs, merge-fusion settings,
the eight-byte histogram budget, NULL count arguments, and the optimizer guard
for the correlated join. The C++ setting-unwind test remains unchanged.

The TPC-H migration copies the original native database and eight Parquet files
without rewriting their storage encodings. Its join, aggregate, sort, pinning,
optimizer, empty-side, and fallback cases retain the original query sequences.
Previously hidden cases remain in a separate `previously-disabled` run. SF10
requires explicit local source bindings; preparation records their hashes.
`tpch-multi-gpu` and `sf10` use the original two-GPU integration profile.

Date-cast cases retain plain scans, native GPU pins, compressed Parquet pins,
NULL removal before compression, bitpack plans, and all timestamp cutoffs.
Three cases intentionally document different CPU and GPU semantics: SQL setup
requires DuckDB to reject the overflowing cast, then compares Sirius with the
original explicit expected rows. Their `known_semantic_divergence` tag
identifies this contract. C++ still checks the compressed-entry census.

Integration profiles preserve the C++ memory pools and operator limits. Their
declared environment enables internal test options and the fused scan gates
armed by the C++ date-cast fixture. Lightweight profiles remain available for
smoke tests and exploratory sweeps. Worker environments are included in reports
and comparison fingerprints.

## Validation scope

Execution of the CI draft is deferred, including checks on two-GPU runners.

All 80 migrated S3 comparisons pass with managed MinIO and fallback disabled.
Their duplicate C++ comparisons are removed; internal assertions remain in C++.
The opt-in Rust MinIO upload/cleanup test also passes on the host.
The retained C++ S3 selection passes all 505 assertions in 20 cases, including
the changed surface checks and tiny/SF1 TPC-H routing checks
(`runs/s3-retained-cpp-host.log`). This completes local S3 validation.
The C++ harness uses disk-backed MinIO and requires sufficient free capacity
on Docker's backing filesystem;
the SQL runner's tmpfs policy does not configure that separate harness.

Preserve data scale, NULL/encoding premises, optimizer settings, execution order,
and GPU requirements when moving a case. Benchmark SQL at a different scale or
with a different fixture does not establish equivalent coverage. Keep the old
assertions until their replacements are verified.

No in-scope direct C++ CPU/GPU result comparison helper remains.
TPC-H comparisons for both GPU counts and SF10 now live in SQL; C++ retains
execution-route and dynamic-filter domain assertions. Execution-route, fallback,
cache-state, and negative-error assertions remain intentionally. The S3 warm-scan
and operator SQL scripts preserve consecutive query executions in one worker.

All 1,692 migration entries resolve to unique SQL IDs in their declared files
(`runs/sqltest-migration-identities-audit.json`). Source anchors exist at the
recorded revision. A title audit's 135 differences consist of 24 split C++ string
literals, 63 section/scenario labels, 29 helper scenarios, and 19 commented CSV
cases; these do not indicate missing source cases.

## Local validation

The migration map contains **1,692 mapped queries**, including 22 previously
hidden queries, four opt-in SF10 cases, 22 original SF1 TPC-H queries, and five
two-GPU partition-memory queries, plus 40 other two-GPU operator queries, 80 GPU-validated S3 queries,
and 19 formerly commented-out CSV queries.
The standard `correctness` run selects
**1,503 cases**: 1,500 migrated queries plus three existing regressions. Compression
and Hive watchdog suites use fixed profiles within that run, not sweeps.
These counts describe the corpus, not SQL feature completeness.

Combined current-artifact GPU validation covers **1,502 matching cases out of
1,503**, including the six added manual-join regressions and four Iceberg
liveness probes. The original batch covered 1,492 matches out of 1,493;
`runs/manual-joins-migration-gpu` and `runs/iceberg-liveness-migration-gpu`
validate the additions. The current dry-run selection still contains 1,503
cases (`runs/sqltest-completion-plan.json`).
`regressions/exact_numeric` fails because native scans do not support
`DECIMAL(38,4)` storage. This existing feature gap remains a failure with
fallback disabled; it has no baseline exception. The full command in
`runs/correctness-final-local` records 1,456 matches, this error, and 36 Iceberg
cases affected by a missing temporary Avro/Iceberg installation. Running the
normal fixture preparation restores those dependencies; all 36 cases then match
in `runs/correctness-final-iceberg-recovery`. The recovery has identical case
fingerprints, runner, runtime, and Sirius artifact hashes. The combined evidence
is recorded in `runs/sqltest-final-validation-summary.json`; the initial failed
report remains intact. Avro `f9d5902` and Iceberg `45163a28` match the earlier
validation versions.

The current-artifact `benchmarks` run completes all 328 native/Parquet checks in
`runs/benchmarks-final-local`. Counts aggregate both storage modes:

| Suite | Checks | Matches | Query errors | Mismatches | Crashes |
| --- | ---: | ---: | ---: | ---: | ---: |
| TPC-H | 44 | 44 | 0 | 0 | 0 |
| TPC-DS | 198 | 78 | 114 | 4 | 2 |
| ClickBench | 86 | 80 | 4 | 2 | 0 |

TPC-DS Q18 and Q22 and ClickBench Q04 mismatch in both storage modes; TPC-DS Q35
crashes in both. Later cases still execute. These failures have no baseline
exceptions, so the command exits with status 1. The report is complete and can
be used for future history comparisons. These benchmark suites remain separate
from the migrated `correctness` selection.

Accumulated CPU runs cover all 1,493 standard cases. The original 525-case batch
is recorded in `runs/migration-*-cpu/`. Additional reports include:

- `migration-tpch-native-cpu-v2`: 207 cases.
- `migration-tpch-empty-matrix-cpu`: 23 native empty-side joins.
- `migration-tpch-fallback-cpu`: four explicit fallback snapshots.
- `migration-tpch-parquet-cpu-v2`: 213 cases, including empty-side joins.
- `migration-tpch-empty-cpu`: six empty-table snapshots and schema comparisons.
- `migration-dynamic-native-original-cpu`: ten dynamic-filter cases.
- `migration-dynamic-sip-cpu`: 26 filter-off/on cases.
- `migration-cast-date-cpu`: 277 date-cast cases.
- `migration-parquet-carrier-cpu`: ten Parquet count-carrier cases.

The fresh extension and C++ test binary build successfully. With test options
explicitly enabled, all 36 dense count-join cases and the 13-million-row MVCC
case match DuckDB. Reports are in `migration-dense-count-integration-gpu` and
`migration-mvcc-multichunk-integration-gpu`. Their C++ result comparisons have
been replaced by execution assertions. The dense count-join C++ selection
passed **1,376 assertions in 58 test cases**.

All 234 native TPC-H cases matched the fresh extension: 232 in
`migration-tpch-native-integration-gpu-v2` and two in
`migration-tpch-native-avg-integration-gpu` after their workers initially failed
GPU allocation. All 213 Parquet cases matched in
`migration-tpch-parquet-integration-gpu-v2`. Additional GPU reports cover:

- `migration-dynamic_filter_native-integration-gpu`: all ten cases, including the allocation retry.
- `migration-dynamic_filter_sip-integration-gpu`: all 26 filter-off/on cases.
- `migration-cast_date-integration-gpu`: all 277 date-cast cases.
- `migration-tpch_empty-integration-gpu`: all six empty-table cases.
- `migration-parquet_count_carrier-integration-gpu`: all ten count-carrier cases.
- `migration-compressed-gate-gpu`: all 32 residency-gate cases; CPU checks also pass.
- `migration-compressed-partition-gpu`: all 11 partition cases; CPU checks also pass.
- `migration-hive_escaped-gpu`, `migration-hive_disabled-gpu`, and `migration-hive_watchdog-gpu`: all 17 Hive cases.
- `migration-iceberg-gpu`: all 36 Iceberg comparisons and original literal results.

The fixed-profile correctness run also passes all 47 compression and Hive
watchdog cases (`fixed-correctness-*-gpu`). The retained multi-format C++ checks
pass **1,786 assertions in 52 cases**.

All 31 pinning comparisons pass CPU and GPU checks. Reports are in
`migration-pin_column_order-*`, `migration-pin_column_merge-*`,
`migration-pin_host_streaming-*`, `migration-pin_zone_map-gpu-v2`, and
`migration-pin_zone_map_native-gpu-v3`. Native fixtures are checkpointed, closed,
and reopened before pinning, preserving the original database lifecycle.
Fixed profiles retain the original 40% GPU budget for column-order/merge tests,
256 MiB limit for host streaming, and 2 GiB limit with 8 MiB scan batches for
zone-map tests. The zone-map profile explicitly enables the test-only pruning
setting. C++ keeps cache metadata, peak-memory, chunk census, and pruning probes.

The five transparent-execution comparisons also pass CPU and GPU checks in
`migration-transparent-{cpu,gpu}-v2`. C++ keeps GPU routing and CPU bypass
counters and prepared-statement lifetime checks. This file was missing from the
CMake test sources; it is now registered and uses the existing persistent-data
fixture. Its stale CPU-lock test contradicted the active lifecycle contract and
was removed: `test_query_lifecycle_slot.cpp` already checks GPU serialization
and CPU bypass. The restored transparent selection passes **268 assertions in
10 cases**, including the existing regex check. The pinning selection passes
**285 assertions in six cases**; one unrelated two-GPU merge case is skipped
on this single-GPU host. The existing GPU-serialization and CPU-bypass lifecycle
checks also pass (**12 assertions in two cases**).

The Hive and Iceberg scripts also pass CPU checks. The Iceberg reader is the
original verified version `45163a28`; Avro reports `f9d5902`. Its 174 fixture
files are checksum-verified and copied into each worker with their original
relative layout. The standalone CPU-reader schema-evolution guard remains in
C++; it was never a CPU/GPU comparison.

These comparisons now live in SQL. C++ retains execution-route counters,
compression/residency assertions, and fixture premises. All TPC-H row comparisons
now live in SQL, including the opt-in cases. Validation uses fixed runner and
extension copies so rebuilding cannot interrupt an active run.

The harness passes **38 Rust tests** and Clippy. SQL formatting and the changed
file pre-commit checks pass; Markdown lint is run separately. The combined C++
selection passes **9,428 assertions in 216 cases**. Validation exposed an
existing unique-join fixture leak: it reset the shared optimizer list to empty,
breaking later MVCC join execution. Its scoped guard now restores the previous
list while preserving the explicit replacement used during the test. All 16
MVCC delete cases also pass in isolation (488 assertions). The newer selection
of TPC-H, dynamic filters, date casts, count carriers, and compression residency
tests passes **22,485 assertions in 433 cases**. After the final partition and
fallback comparison removals, the focused selection passes **787 assertions
in 13 cases**. SF10 and two-GPU execution remain deferred to suitable hosts;
S3 transport validation remains outstanding.

The CI changes are a local draft and have not run on GitHub. Nightly publication
remains deferred.

The 29 local fallback-mix cases and seven injected runtime-fallback cases pass
CPU and GPU checks (`migration-fallback_mix-{cpu,gpu}-v2` and
`migration-runtime_fallback-{cpu,gpu}-v2`). The final three projection snapshots
also pass with their original first-row order (`migration-fallback-projections-gpu`).
Named connections preserve the older transaction snapshot during a concurrent
commit. C++ retains route counters, error handling, and subprocess watchdogs.

All 25 vector comparisons pass on both engines (`migration-vector_search-*`).
Engine-specific views compare Sirius's explicit GPU search function with
DuckDB's exact distance queries. Distance comparisons retain the original
float tolerances and deterministic ordering. The 200,000-row multi-chunk
fixtures retain their 4 KiB pinning batch setting. The outer query explicitly
allows fallback because the Sirius-owned table function executes GPU search
inside its function body.

The initial retained fallback/vector C++ selection passed 41 of 42 cases.
The ANN-underfill fixture queried a zero centroid belonging to an empty IVF
list. It now chooses a populated list smaller than k with a unique centroid,
then uses that centroid as the query. Padding, nonempty-result, range, and
uniqueness assertions remain, with an added exact list-size assertion.

The original SF1 TPC-H run passes all 22 cases with its preserved goldens in
`migration-tpch-legacy-goldens-{cpu,gpu}`. All eight original `.tbl` inputs and
22 CSV goldens are checksum-pinned. SQL checks compare typed golden rows in
order with DuckDB, then the runner compares ordered Sirius results with DuckDB.
The original legacy test, result-generation SQL, and issue #56 test are removed.
The 22 original goldens are moved byte for byte into `fixtures/goldens/tpch`;
there is one copy of each. Obsolete legacy test commands and `LOAD_TESTS`
registration are removed; legacy engine code is unchanged.
`setup_test_datasets.sh --tpch-only` prepares these inputs independently of
ClickBench. This larger dataset remains an explicit `legacy-tpch` run.

The partition-memory control passes all five repeated joins on CPU and GPU
(`migration-partition-memory-{cpu,gpu}`). Its fixed profile preserves the
512 MiB GPU limit, 20 MB scan batches, 256 MB build-table limit, eight
2-million-row fact files, and 200,000-row dimension table. The result-only
single-GPU C++ case is retained because it exercises the legacy explicit
entrypoint. The two-GPU suite passes CPU validation in
`migration-partition-memory-mgpu-cpu`; its original legacy-entrypoint C++
comparison also remains. Both SQL suites check
explicit count and sum snapshots on every iteration.
The restored original single-GPU C++ control passes 97 assertions at its default
scale and memory limit (`runs/sqltest-restored-partition-control.log`).

The pin-insert audit found two comparison calls inside a commented-out block
for the obsolete ARRAY-rejection contract. That block is removed; the active
ARRAY insert-delta tests and their SQL replacements remain.

After these changes, the C++ binary rebuilds successfully and the retained
pin-insert and multi-chunk vector selection passes **788 assertions in 26 cases**
(`runs/sqltest-retained-pin-vector-checks.log`). The corrected fused ANN
underfill fixture passes 230 assertions in
`runs/sqltest-ann-deterministic-first.log`.
All changed-file pre-commit hooks, the full SQL formatting check, and local
workflow validation pass. The runner's correctness dry run confirms 1,493
cases. GitHub CI execution and two-GPU validation remain outstanding.

All 40 sort, grouped-aggregate, hash-join, and broadcast operator comparisons
pass CPU validation in `migration-operators-mgpu-cpu`. Together with the five
partition-memory queries, `operators-multi-gpu` selects 45 cases. These preserve
16 original operator cases, their Parquet file counts and row counts, fixed
profiles, and consecutive executions. The old helper's default reference was
another GPU execution; each such call becomes two SQL comparisons against
DuckDB. Existing C++ assertions remain because their helper exercises the
legacy entrypoint, including per-device scheduling and broadcast/filter
publication checks. Transparent SQL coverage does not replace that entrypoint.

The shared `GpuExecutionFixture` result comparator has no remaining callers
and is removed. Its seven standalone C++ comparator tests are replaced by
Arrow comparator regression cases covering exact numeric strings, NULLs,
near-zero and relative tolerances, sign errors, and non-finite values. The
retained C++ tests compile after the removal; GPU routing and fallback helpers
remain in the fixture. The runner/formatter checks pass all 38 Rust tests in
`runs/sqltest-validator-regressions.log`.


The `s3` run now declares a runner-managed MinIO service through Testcontainers.
Object uploads use the `rust-s3` client and existing checksum-verified Parquet
fixtures. Its initial 22 tiny TPC-H queries preserve the original shared sequence,
query variants, fixed memory configuration, and Q1 floating-point tolerance.
Q1 uses its deterministic group-key order for approximate comparison; other
queries compare exact multisets. No query failures are automatically retried.
All 22 pass CPU validation in `runs/migration-s3-tpch-cpu`. This does not establish
S3 or GPU coverage. The actual S3 attempt records an infrastructure failure because
`/var/run/docker.sock` is absent (`runs/migration-s3-minio-requirement`). All
original S3 C++ comparisons and internal assertions remain until service/GPU
validation. The opt-in 1,001-object pagination query now has a separate SQL run.
Routing, fallback diagnostics, and planner metadata stay in C++.
Legacy explicit-entrypoint C++ checks remain outside this migration.

The managed-service changes pass 41 Rust runner/formatter tests. One opt-in test
checks real MinIO upload key fidelity and container cleanup; it requires Docker
and has not run on this host. Standard tests verify manifest errors, profile
binding, service fingerprints, and CPU-only operation without Docker. The initial hidden
TPC-H CPU audit found two reference timeouts in the full outer double-inequality
join (one native, one Parquet); the other 14 hidden TPC-H queries pass.


The previously commented-out CSV comparisons are restored as 19 runnable SQL
cases in `csv_disabled`, selected through `previously-disabled`. They preserve
the original Parquet-to-CSV exports, inferred CSV types, SQL, and floating-point
tolerances. All 19 pass CPU validation (`runs/migration-csv-cpu`); all 19 fail
Sirius with fallback disabled because `read_csv` is unsupported
(`runs/migration-csv-gpu`). These remain visible feature gaps, not passing
fallback tests or baseline exceptions. The obsolete commented C++ fixture and
queries are removed; no active C++ assertions were removed.

There are also 34 draft S3 surface queries covering encoded keys, globs, Hive
partitions, footer probes, and repeated warm scans. Their original object layout
is declared as checksum-verified file copies, and four fixed suite profiles
preserve the original memory and scan settings. CPU validation passes all 56
S3 queries (`runs/migration-s3-surface-cpu`), including 27 original literal-result
assertions in the new surface cases. Suite manifests now declare per-engine
literal substitutions for file roots, preserving Sirius's literal-path recognizer.
Original S3 C++ assertions remain intact until Docker/GPU validation succeeds.
The runner will not add legacy `gpu_execution('…')` or `gpu_processing`
entrypoints; legacy C++ internals are excluded from this migration.


Suite literal substitutions pass all 45 Rust runner/formatter tests (one Docker
integration test remains opt-in), Clippy, and the applicable pre-commit hooks.
All 56 S3 reference cases pass again with literal paths in
`runs/migration-s3-substitutions-cpu`. A local Parquet GPU probe passes with
different per-engine paths containing quotes (`runs/substitutions-local-gpu`);
its saved SQL confirms both values are escaped and expanded correctly. This
probe exercises transparent execution with fallback disabled, not S3 transport.
The standard correctness plan remains 1,493 cases.


The transparent SF1 S3 suite passes all 22 CPU comparisons in
`runs/migration-s3-sf1-cpu`. Its original `dbgen(sf=1)` fixture uses Snappy, with
the original 2/4/16 GiB GPU/host/disk profile. The no-match glob error passes
CPU validation in `runs/migration-s3-no-match-cpu`. Both remain unvalidated on
S3/GPU because this host lacks a Docker daemon. Legacy explicit-entrypoint
cases, including TLS and large-lineitem variants, remain unchanged.

Counted object recipes declare the 1,001-file pagination layout without listing
every key. CPU workers receive the same layout privately. Runner checks cover
key collisions, invalid paths, zero counts, completion, replay, and changed
recipe fingerprints. All 48 Rust tests pass; the real MinIO integration test
still requires Docker. A source audit also corrected 63 multi-format source
anchors against the recorded revision, without changing their SQL.

The pagination case passes CPU validation in `runs/migration-s3-pagination-cpu`,
including its four original literal results. Both workers contain exactly 1,001
Parquet files with identical source hashes. This validates the fixture and SQL
oracle; MinIO LIST pagination still needs S3/GPU execution.

All 57 regular S3 reference cases pass with private object staging in
`runs/migration-s3-object-copies-cpu`; together with SF1 and pagination this
covers 80 S3 CPU cases. The standard correctness plan remains 1,493 cases.


All 14 previously hidden TPC-H cases with working CPU references pass GPU
comparison (`runs/sqltest-hidden-tpch-gpu-audit.json`). Their C++ tests now
retain execution-route assertions; SQL owns the result comparisons. The two
full-outer double-inequality cases now use equivalent INNER JOIN CPU oracles
in SQL and in the retained C++ CPU-bypass assertions. Their Sirius queries
keep the original FULL JOIN; their C++ cases remain hidden.

The 14 hidden C++ cases retain `[.]` and now include `[integration]` so explicit
selection initializes Sirius. Together with both ANN-underfill cases, they pass
**5,662 assertions in 16 cases** (`runs/sqltest-ann-hidden-cpp-v2.log`). The
corrected fused ANN fixture also passes ten fresh processes in
`runs/sqltest-ann-deterministic-repeat.json`.

The CI SQL job now includes the original 44 native/Parquet TPC-H variants on
two GPUs, alongside the operator cases. Its exact selection passes CPU
validation (`runs/ci-tpch-mgpu-cpu`), and workflow lint passes. Execution on
the CI GPU runners remains unverified. The default local MinIO attempt records
`Socket not found: /var/run/docker.sock` in `runs/default-docker-s3-check`.
SF10 inputs were found at `/data/tpch/sf10/snappy`. All eight files were bound
explicitly during preparation, which recorded their hashes. All four CPU
reference checks pass in `runs/migration-sf10-default-cpu`. Their original
two-GPU requirement remains; this host has only one GPU.

The two-GPU requirement is a property of those runs, not a requirement for local
validation. Their C++ cases retain GPU execution, CPU bypass, fallback, and
dynamic-filter domain checks while SQL owns row comparisons and tolerances.

The CI specialized SQL steps run after a correctness failure when fixture
preparation succeeded. Failures still fail the job; later TPC-H and two-GPU
reports are not silently skipped. Workflow lint passes locally.

CI also selects all 80 transparent S3 cases through `s3`, `s3-sf1`, and
`s3-pagination`. The build job checks their local CPU references without Docker;
the GPU job starts managed MinIO services and preserves reports for each run.
A failure in one S3 run does not skip the others. These CI additions have not
been executed on GitHub.

A temporary, isolated Podman Docker API was reachable on this host. The
manifest-pinned MinIO image could not be pulled because connections to Docker
Hub timed out, including the configured retries. The temporary API was stopped;
S3/GPU and real container lifecycle validation remain unverified.

Tracing `filter_nulls/between-and-in-with-nulls/002` reproduces the Rust TLS
shutdown messages seen in worker logs. Both worker processes nevertheless exit
with status 0 and the result matches DuckDB (`runs/sqltest-worker-shutdown-filter.trace`).
These messages are not a masked process crash in the observed case.

After moving the remaining two-GPU/SF10 TPC-H comparisons into SQL, the C++
binary rebuilds and the native/Parquet Q1 and Q4 routing checks pass locally:
**202 assertions in four cases** (`runs/sqltest-tpch-routing-local.log`).
Unavailable two-GPU variants are not counted as local validation. Changed-file
pre-commit hooks, workflow lint, and whitespace checks pass.

The two hidden full-outer double-inequality references apply their filters above
DuckDB's FULL IE_JOIN over 600,572 lineitem rows and 80,000 partsupp rows.
Both WHERE predicates reject unmatched rows, so an INNER JOIN oracle preserves
the result and returns the 1,000 rows in under a second. A small fixture with
duplicates, NULLs, and unmatched rows confirms multiset equality. This oracle
change is applied through engine-specific temporary views using existing
SQLLogicTest statements. Both CPU-only runs return all 1,000 rows and pass
(`runs/hidden-oracle-{native,parquet}-cpu`). Both GPU reports are complete and
record `NESTED_LOOP_JOIN` out-of-memory after exhausting task retries
(`runs/hidden-oracle-{native,parquet}-gpu`). The saved reproductions retain INNER
JOIN for DuckDB and the original FULL JOIN for Sirius. Fallback remains disabled,
and neither error has a baseline exception. The reports now expose engine
failures rather than CPU reference timeouts; they do not claim matching results.
`runs/sqltest-hidden-oracle-validation.json` records the report audit.

The C++ TPC-H result comparator and its unused numeric/row helpers are removed.
All TPC-H cases retain their internal execution checks. The two hidden cases
also use the equivalent CPU query for their CPU-bypass assertions and retain
their original GPU query, hidden status, and dynamic-filter invariants.


The standalone manual join script is migrated as six self-contained queries in
`regressions/manual_joins.slt`; all six match DuckDB with fallback disabled.
The four Iceberg conformance files now include same-connection liveness probes,
explicit checkpoint policy, and 90-second query timeouts. Their snapshots match
all four pyiceberg expectations. All 40 Iceberg queries match DuckDB, including
the four new probes; the final eight conformance queries also pass independently.
The old manual script and Python conformance runner, including its separate CI
step, are removed. Evidence is in `runs/manual-joins-migration-gpu`,
`runs/iceberg-liveness-migration-gpu`, and `runs/iceberg-conformance-final-gpu`.

The corpus now has 1,859 query IDs. Of its 66 suites, 44 generate inputs through
SQL, 21 reuse existing or supplied file fixtures, and ClickBench downloads a
checksum-pinned input. Removing 22 duplicate TPC-H golden files avoids adding
70,892 duplicate lines (7,515,595 bytes). The single retained copies now live
under `fixtures/goldens/tpch` with the original verified checksums. No expected-output checks were removed.


The legacy SQL infrastructure cleanup removes `test/sql/*.test`, the modified
TPC-H `.test` file and its unused goldens, both old golden-generation scripts,
and the hardcoded legacy Python test. `LOAD_TESTS`, the extension-template test
rename, and the obsolete formatter exclusion are removed. The retained 22
TPC-H goldens move unchanged into the new corpus. Engine sources, C++ legacy
internals, and performance/profiling tools are untouched. The removal and move
inventory is recorded in `runs/sqltest-legacy-infra-removal.json`.
All 217 declared file sources still exist and match their checksums. The relocated
goldens pass all 22 CPU checks in `runs/goldens-relocated-cpu`; CMake configuration
without `LOAD_TESTS` and the changed-file formatting/lint checks pass.

The managed MinIO image now uses `quay.io/minio/minio` with the same explicit
release tag. Docker Hub's MinIO image was withdrawn; the upstream publication
script also publishes the `cpuv1` release to Quay. The full image reference is
recorded in service metadata. This host's bounded Quay probe still times out
(`runs/sqltest-quay-registry-probe.log`), so real container and S3/GPU validation
remain outstanding; no daemon or container is left running.
The retained C++ S3 container helper uses the same registry and tag. The registry
change passes all 48 Rust runner/formatter tests, Clippy, the C++ helper build,
and changed-file hooks. The real MinIO lifecycle test remains unexecuted.

The first host-side Docker run (`runs/s3-host-3RIleq`) successfully starts MinIO,
but none of its 80 comparisons execute. The 57 regular and 22 SF1 cases fail or
are blocked by missing `libcuvs.so`; the build environment's `.pixi` is a private
Docker volume, not the host directory. Pagination fails uploading its first
object with `XMinioStorageFull` on Docker's nearly full backing filesystem.
The `sqltest-host` launcher now discovers that volume's readable host path and
checks shared libraries before preparing fixtures. The four S3 service recipes
declare private 2 GiB tmpfs data mounts; the policy is recorded in results.
The subsequent `runs/host-h48cmwm5` reports validate all 57 regular S3 cases and
the pagination case with the pinned extension and managed MinIO. SF1's 22 cases
do not reach comparison: view binding requires GPU execution, but its initial
SET statement is reset before the separate CREATE VIEW statement. The corrected
fixture places SET and CREATE VIEW together for Sirius, matching the tiny
fixture's setup. All 22 corrected CPU checks pass in `runs/s3-sf1-setup-fixed-cpu`;
the S3/GPU rerun remains outstanding.

The host launcher is now the Rust `sirius-sqltest-host` binary in the runner
package; its Python implementation and tests are removed. It shares the runner's
identifier parser and does not link DuckDB, allowing it to establish library
paths before starting the runner. It preserves NixOS driver discovery, Docker
mount discovery, dependency preflight, and sequential runs. All 54 Rust tests,
Clippy, and formatting checks pass.

The host-side `runs/host-H8hlxp/s3-sf1` report validates the corrected SF1 fixture:
all 22 queries match. Together with `runs/host-h48cmwm5/{s3,s3-pagination}`, all
80 migrated S3 cases pass with the same extension/runtime hashes and fallback
disabled. `runs/sqltest-s3-retirement/audit.json` records report hashes, verifies
that all 80 IDs map to the migration inventory, and checks retained C++ assertions.
The S3 TPC-H C++ tests now keep only execution-route and GPU-off rejection checks.
The surface fixture removes its CPU comparison helper and ten result-only cases,
consolidates literal-key LIST assertions, and preserves REST routing, GPU counters,
footer/warm-cache probes, cache identity, and negative-error checks. Its 55 other
test bodies are unchanged apart from whitespace. The old C++ pagination upload
recipe and Makefile selector are removed; the SQL manifest owns that fixture.
The rebuilt `sirius_unittest` compiles and links, and its retained S3 test
registrations are verified. The host-side `runs/s3-retained-cpp-host.log` now
validates all 20 retained cases with 505 passing assertions against managed MinIO,
including both tiny and SF1 TPC-H routing checks. Changed-file hooks pass, and
engine/performance sources are unchanged.

The host-side `runs/sqltest-minio-lifecycle-host.log` records a passing
`services::tests::managed_minio_uploads_exact_keys_and_cleans_up` test
(1 passed, 0 failed, 2.80 seconds), verifying exact upload keys, counted copies,
and managed container cleanup.

The final local audit confirms 1,859 unique corpus IDs and 1,692 migration
entries with no missing file/ID pairs. All 54 regular Rust tests pass; the
separately run MinIO lifecycle test brings the total to 55. The 80 S3 SQL
comparisons and 20 retained C++ S3 cases pass. CI execution and two-GPU GPU
validation remain deferred; the documented engine failures remain visible in
reports rather than being suppressed by baseline exceptions.
