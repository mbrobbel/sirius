# Local SQL correctness suites and regression runner

Sirius needs a unified SQL correctness suite to expose support gaps, detect regressions against DuckDB, and turn issue reproductions into small tests. Existing C++ integration tests provide valuable execution checks, but adding broad SQL coverage should not require writing C++ fixtures.

The proposed interface follows [DataFusion's SQLLogicTest workflow](https://github.com/apache/datafusion/blob/main/datafusion/sqllogictest/README.md): self-describing `.slt` files contain setup statements and queries, with shared fixtures through `include`. The Rust runner uses [sqllogictest-rs](https://github.com/risinglightdb/sqllogictest-rs), DuckDB's Rust bindings, and Arrow IPC, sorting, and equality. Custom code handles Sirius artifacts, execution profiles, known gaps, and reporting.

The initial corpus contains all TPC-H, TPC-DS, and ClickBench query IDs, using small reproducible datasets, plus focused regressions. Each query runs against a CPU reference and a loaded Sirius `.duckdb_extension` using the same DuckDB runtime. Small regressions can also retain expected output generated from DuckDB using `complete`. One/two-GPU profiles and native-table/Parquet fixtures exercise different execution paths without duplicating queries. Suites are discovered from directories, with optional per-suite TOML metadata for fixture dependencies and supported storage modes. A typed TOML manifest declares SQL fixture recipes, storage variants, GPU profiles, and named configuration axes. Runs expand explicit choices into Cartesian-product sweeps: for example, fix the optimizer configuration while sweeping storage and GPU profiles, or sweep all three axes. New suites and axes require no benchmark-specific Rust code; conflicting settings and invalid references fail before execution.

This measures correctness, not performance. Benchmark workloads provide useful realistic shapes, but passing them does not establish SQL feature completeness. Reports also surface empty results, errors, mismatches, crashes, timeouts, and missing requirements. Known gaps remain runnable and require explicit expectations and issue references; new failures and newly passing cases require review.

The standard correctness run executes each suite once per supported storage mode,
using a fixed profile. Suites with regression-specific memory limits, batch sizes,
or startup environment declare their own profile. Configuration sweeps remain
separate exploratory runs. Cases that require two GPUs, SF10 data, or external
services have explicit named runs and requirements.

Migration preserves the existing fixtures, data scale, query sequences, and
expected rows before retiring C++ result comparisons. C++ retains assertions
about GPU execution counters, fallback routes, cache state, and concurrency.
Only transparent Sirius execution is supported. Old SQL correctness runners,
including legacy SQL tests and extension test registration, are removed while
engine code and performance/profiling tools remain unchanged. Two-GPU suites run on suitable
CI infrastructure and do not require validation on a single-GPU development host.
SQL suites use named connections and explicit checkpoints for transaction
visibility, private scratch directories for generated files, and checksum-verified
fixture copies. User-provided datasets require explicit bindings and recorded
hashes. Intentional fallback is declared per query; ordinary queries disable it.

The default optimizer choice preserves the loaded artifact's settings. Other presets supply explicit replacement lists, allowing sweeps to expose unsupported optimizer combinations. Each configuration gets distinct results and reproduction files; the CPU reference retains its own defaults unless explicitly configured otherwise.

The initial runner uses existing extension artifacts without adding diagnostics to Sirius. Disabling CPU fallback alone cannot prove GPU execution. Results therefore distinguish the fallback policy from execution evidence, while retained C++ checks verify routing. A future structured diagnostics interface could provide stronger execution evidence within SQL runs.

A follow-up nightly GitHub Actions job could test the latest successful `dev` Distribution artifact and publish summaries, JSON/JUnit reports, and reproduction files. Local reports already identify the extension, runtime, suite, fixtures, and execution configuration. Comparisons against previous complete reports distinguish engine regressions from changed tests or datasets. CI artifacts could initially provide retention-limited history; a later GitHub Pages dashboard could consume the same JSON and retain longer-term trends.

The current scope is the local runner, corpus, and correctness CI: prepare data, discover suites, run explicit configuration sweeps or individual cases, generate regression snapshots, and inspect results. External directories of generated tests use the same CPU/GPU comparison and reporting path, so a future fuzzer can emit self-contained, replayable cases with recorded seeds. Fuzz generation and shrinking, nightly automation, broader dataset sweeps, additional platforms, verified GPU execution in SQL reports, and the dashboard are follow-up work. Existing C++ tests remain valuable for assertions that require internal engine APIs.
