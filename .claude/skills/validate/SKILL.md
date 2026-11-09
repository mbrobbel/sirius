---
name: validate
description: >
  Use this skill when a Sirius query returns wrong results, missing rows, extra rows, or incorrect
  values compared to DuckDB CPU. Uses row counts, data inspection, and stream diagnostics to narrow
  down possible causes.
argument-hint: [sql-query-or-file]
disable-model-invocation: true
---

# Validation Error Analyzer

Diagnose incorrect query results by comparing Sirius GPU output against DuckDB CPU baseline, narrowing down possible faulty operators, and investigating the root cause.

**Reference:** See `.claude/skills/_shared/build-and-query.md` for build and logging context. Use the current repository instructions for execution, authorization, and change tracking; do not create checkpoint commits unless requested.

## Workflow

1. **Gather context:**
   - Use the supplied query, data location, and reproduction steps; ask only for details needed to proceed.
   - Preserve any user changes and keep temporary instrumentation easy to remove.

2. **Compare CPU and GPU results:**
   Run both queries in the same shell so they share the temporary directory. Replace the database
   path and SQL with the supplied reproduction, and retain the printed directory path for analysis:
   ```bash
   result_dir=$(mktemp -d)
   printf 'Results and logs: %s\n' "$result_dir"
   pixi run sirius-duckdb/build/release/duckdb <db_path> -c "SELECT ...;" > "$result_dir/baseline_result.txt"
   export SIRIUS_LOG_LEVEL=trace
   export SIRIUS_LOG_DIR="$result_dir/log"
   mkdir -p "$SIRIUS_LOG_DIR"
   pixi run sirius-duckdb/build/release/duckdb -unsigned -cmd "LOAD 'sirius-duckdb/build/release/extension/sirius/sirius.duckdb_extension';" <db_path> -c "SELECT ...;" > "$result_dir/gpu_result.txt"
   ```
   Confirm the query actually ran on the GPU; transparent execution can fall back to CPU. Compare
   results as multisets unless the query specifies ordering, and account for types and floating-point
   tolerances. Preserve duplicates when comparing.

3. **Handle inconsistent results:**
   If results match on first run but user reports inconsistency:
   - Repeat the entire comparison block (up to 10 times) until a wrong result appears. Each
     repetition creates a fresh result directory; retain and label each directory as good or bad.
   - Record both good run and bad run logs for comparison
   - Flag as potential race condition

4. **Phase 1: Row count analysis with `tools/parse_pipeline_log.py`**
   Substitute the printed result directory path:
   ```bash
   pixi run python3 tools/parse_pipeline_log.py <result_dir>/log/sirius_*.log
   ```
   - Check whether the parser recognizes the current log format and yields relevant row counts.
     If it does not, inspect the trace logs directly or use suitable temporary instrumentation; empty
     output does not establish that counts match.
   - Compare equivalent operator boundaries between runs. A divergence narrows the investigation;
     it does not by itself identify the root cause.

5. **Phase 2: Data validation**
   If row counts match but results differ, inspect the data:
   - Insert debug utility calls at operator boundaries (see Debug Utilities section):
     - `sirius::debug_checksum(*batch, stream)` -- per-column xxhash_64 fingerprint
     - `sirius::debug_stats(*batch, stream)` -- per-column min, max, sum
     - `sirius::debug_head(*batch, 5, stream)` -- first 5 rows for visual inspection
   - Compare equivalent GPU batches from good/bad runs where possible. These utilities require GPU-tier batches; CPU results are not directly comparable as batches.
   - For deterministic failures, compare row counts and per-column min/max/sum at a matching
     operator boundary with the equivalent DuckDB CPU subquery. Account for types, nulls, and
     floating-point tolerances; do not compare checksums from different algorithms.
   - Use `sirius::debug_diff(*batch_a, *batch_b, stream)` only for equivalent GPU-tier batches at the same pipeline point. Different batch boundaries or row ordering can affect comparisons.

6. **Phase 3: Deep dive into the suspected operator**
   Once a likely operator has been identified:
   - Read its implementation (both the `.cpp` and `.cu` files)
   - Add more granular debug utility calls inside the operator (e.g., `debug_head` after each transform)
   - Rebuild and re-run to understand exactly where data goes wrong
   - Suggest a fix

7. If a fix is within the requested scope, apply it, rebuild, and verify against the CPU baseline.
   Remove temporary instrumentation and report what was verified and any remaining uncertainty.

## Common Validation Error: Stream Synchronization / Garbage Data

One possible cause of wrong results is **reading garbage data due to CUDA stream synchronization issues**. This happens when an operator's output is read before the GPU kernel writing it has finished, because the kernel ran on a different stream than expected.

**Root cause:** Sirius uses a stream-per-thread model, but some cuDF operations internally use `cudf::default_stream()` instead of the caller's CUDA stream. When Sirius calls `stream.synchronize()` on its own stream, this does NOT synchronize `cudf::default_stream()`, so data written by cuDF on the default stream may not be visible yet -- resulting in garbage or stale data being read.

**Reference commit:** `69e4c6cf` fixed instances of this pattern where `cudf::default_stream()` was accidentally used.

**Typical symptoms:**
- Data looks like **garbage**: very large numbers, zeros, or partial/stale results rather than logically wrong values
- Wrong results that are **intermittent** (sometimes correct, sometimes wrong)
- Results that change between runs with no code changes
- **More likely to reproduce at larger batch sizes** (GBs) -- small data may complete before the race window opens
- Results that become correct when GPU concurrency is reduced (fewer threads)

**How to diagnose:**

1. **Quick check with `stream_check`:** Use the `stream_check` LD_PRELOAD library (`utils/stream_check/`) to detect default stream usage at runtime. It intercepts `cudf::get_default_stream()` and logs a full stack trace whenever the default stream is accessed from a monitored thread.

   In an already configured build tree, enable the optional target and build it. Use the emitted
   library path for `LD_PRELOAD` (normally `build/release/utils/stream_check/libstream_check.so`):
   ```bash
   pixi run cmake -S . -B build/release -DENABLE_STREAM_CHECK=ON
   pixi run cmake --build build/release --target stream_check

   # Run the query with stream_check preloaded
   pixi run env LD_PRELOAD=<absolute-path-to-libstream_check.so> sirius-duckdb/build/release/duckdb -unsigned -cmd "LOAD 'sirius-duckdb/build/release/extension/sirius/sirius.duckdb_extension';" <db_path> <<'EOF'
   <QUERY>;
   EOF
   ```

   Check the output in `default_stream_traces.log`. Each entry shows the **full call stack** where `cudf::get_default_stream()` was called -- these are the code paths that need to use the correct per-thread stream instead.

   **Note:** `stream_check` is already integrated into Sirius via `src/util/stream_check_wrapper.cpp`. The GPU pipeline executor threads automatically enable detection. If `libstream_check.so` is not preloaded, the wrapper gracefully no-ops.

2. **Check with nsys:** Profile the query with `nsys` and examine the stream IDs:
   ```bash
   pixi run nsys profile --stats=true -o /tmp/validate_profile sirius-duckdb/build/<preset>/duckdb -unsigned -cmd "LOAD 'sirius-duckdb/build/<preset>/extension/sirius/sirius.duckdb_extension';" <db_path> <<'EOF'
   <QUERY>;
   EOF
   ```
   Look at the CUDA stream IDs in the trace. If `cudf::default_stream` (typically stream 0 or the per-thread default stream) appears where it shouldn't, that's the smoking gun.

3. **Narrow to the faulty operator:** Use Phase 2 (data checksums) to identify which operator produces the first mismatch. Print `sum()`/`max()`/`head(10)` of each operator's output and compare against the correct run.

4. **Confirm with `cudaDeviceSynchronize()`:** Once the faulty operator is identified, insert `cudaDeviceSynchronize()` calls inside that operator -- before reads and after writes:
   ```cpp
   cudaDeviceSynchronize(); // [SIRIUS_DIAG] sync before read
   // ... the suspected read/write ...
   cudaDeviceSynchronize(); // [SIRIUS_DIAG] sync after write
   ```
   - If the wrong result **disappears** with `cudaDeviceSynchronize()`, that supports a synchronization or timing hypothesis but does not prove the root cause.
   - Then narrow down: remove `cudaDeviceSynchronize()` calls one by one to find the exact operation that needs proper stream synchronization.

5. **Fix:** Replace `cudf::default_stream()` with the correct per-thread CUDA stream in the faulty code path. Or ensure the operation explicitly synchronizes the stream it actually uses. **Never leave `cudaDeviceSynchronize()` in production code** -- it serializes all GPU work and destroys performance. It is only a diagnostic tool.

## Debug Utilities

Sirius provides structured debug utility functions in `src/debug_utils.hpp` (implementation in `src/debug_utils.cpp`). **Always use these instead of ad-hoc `SIRIUS_LOG_TRACE` checksum patterns.** All functions output to `SIRIUS_LOG_DEBUG` with `[SIRIUS_DIAG]` prefix, are thread-safe (buffered single-call output), and are wrapped in try/catch (never crash the pipeline).

### Function Signatures

```cpp
#include "debug_utils.hpp"

// Per-column xxhash_64 fingerprint -- deterministic across runs
sirius::debug_checksum(batch, stream);
sirius::debug_checksum(batch, stream, {"col_a", "col_b"});

// Per-column min, max, sum for numeric columns (GPU-side, no host copy)
sirius::debug_stats(batch, stream);

// First N rows in aligned-column or CSV format
sirius::debug_head(batch, /*n=*/10, stream);
sirius::debug_head(batch, /*n=*/5, stream, sirius::DebugFormat::CSV);

// N randomly selected rows (same formatting as debug_head)
sirius::debug_sample(batch, /*n=*/10, stream);
sirius::debug_sample(batch, /*n=*/5, stream, sirius::DebugFormat::ALIGNED, {}, 50, /*seed=*/42);

// Compare two batches: schema check, row count check, per-column value diff
sirius::debug_diff(batch_a, batch_b, stream);
sirius::debug_diff(batch_a, batch_b, stream, /*max_diff_rows=*/20, /*max_rows=*/1'000'000);

// Schema metadata (column names, types, null counts, row count)
sirius::debug_schema(batch, stream);

// Per-column null counts and percentages (zero GPU cost)
sirius::debug_nulls(batch, stream);
```

### Usage in Validation Workflow

**Phase 2 (data validation):** Instead of inserting manual `SIRIUS_LOG_TRACE("[SIRIUS_DIAG] operator_name checksum: sum={}, max={}, first_row={}", ...)` statements, insert these debug utility calls at operator boundaries:

```cpp
// At operator output boundary (e.g., after Execute() or Sink()):
sirius::debug_checksum(*output_batch, stream);  // stable hash per column
sirius::debug_stats(*output_batch, stream);     // min/max/sum per column
sirius::debug_head(*output_batch, 5, stream);   // first 5 rows for visual check
```

Compare equivalent GPU batches from good/bad runs to narrow down the suspected operator.

**Comparing two operator outputs directly:**
```cpp
// Compare equivalent batches from two runs at the same operator boundary:
sirius::debug_diff(*batch_a, *batch_b, stream);
// Output: per-column diff count and first 10 differing row indices
```

**Sampling rows from large batches:**
```cpp
// Check random rows (catches bugs not visible in first rows):
sirius::debug_sample(*batch, 20, stream);
```

## Key Design Decisions

- `tools/parse_pipeline_log.py` can summarize row counts when its patterns match the log format
- Data checksum insertion uses `[SIRIUS_DIAG]` tagged log statements for easy cleanup
- For inconsistent results, the skill has patience to run many times until it catches a bad result
- Consider stream synchronization when results are intermittent or contain garbage data, alongside other possible causes
- `stream_check` (LD_PRELOAD library in `utils/stream_check/`) can help find default stream usage

## Scope

Analyze active code in both `namespace sirius` and `namespace duckdb`; see shared build-and-query.md.
