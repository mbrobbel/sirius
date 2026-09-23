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
