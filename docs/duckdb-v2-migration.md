# DuckDB v2 migration

## First deliverable

Build an out-of-tree `sirius.duckdb_extension` against the unmodified DuckDB v2
revision `561522aea03e400bd20adc64fcc63e78b8721f3f`. Load it into a separately built
host at that revision and execute supported SQL on the GPU. The extension must
bundle its engine, RAPIDS libraries, CUDA runtime, and JIT dependencies. Its only
external runtime dependencies may be the NVIDIA driver (including NVML) and
ordinary Linux/C++ runtime libraries.

The first validation target is Linux x86_64, CUDA 13, SM 120. This deliverable uses
the C++ extension API and requires matching DuckDB revisions. It does not yet
provide a stable host ABI or choose between CUDA 12 and CUDA 13 at load time.

No custom DuckDB Python package is required. A vanilla Python distribution can
load the extension when its DuckDB revision and ABI match the extension.

## Branch sequence

1. `v2/build-base`: start from `build/sirius/06.1-standalone-build`
   (`9bd6ddad26dbda3461587e63b647535e0193f10d`) and integrate main at
   `4e641a71b0ff03abbf331fb5ccf2d0f20816aa21`. Keep the legacy engine deletion
   and cardinality-preserving plan copy. **Completed** in `a1a5cd864`.
2. `v2/wrapper-import`: import `sirius-db/sirius-duckdb` at
   `ca1297c79bb4378f2a19464c5a7e61214e2812d9`, retaining its history and v2 host
   checkout. Preserve the installed-engine consumer from the build split.
   **Completed** in `f17ed2f2a`.
3. `v2/engine-port`: pin both DuckDB checkouts to the target v2 revision. Adapt
   typed identifiers/indexes, query results, prepared execution, scan filters,
   native storage scans, Parquet/multifile scans, registration, and Substrait
   ingestion. Preserve transaction-local CPU fallback and runtime cleanup.
   **Completed** in `bc50fa066`, validated on CUDA 13 / SM 120.
4. `v2/bundled-extension`: complete the static dependency closure, independent
   wrapper build, and packaging checks. Validate the artifact in a clean runtime
   environment. **Implemented and validated** on CUDA 13 / SM 120. Push these
   branches to the personal fork; do not open PRs.

## Acceptance checks

- Build the standalone engine and install its CMake package before building the
  wrapper. Build an independent DuckDB shell with no statically registered Sirius.
- Inspect ELF `NEEDED`, RPATH/RUNPATH, exported symbols, and extension metadata.
  Reject shared engine, DuckDB, RAPIDS, CUDA toolkit/JIT, OpenMP, NUMA, and network
  client library dependencies outside the runtime allowlist.
- Load the extension into the independent host. Exercise native checkpointed
  tables and Parquet, including filters, projection, aggregation, joins, sorting,
  and limits. Compare with CPU results and prove GPU execution, with runtime
  fallback disabled for supported-query checks.
- Test unsupported-query fallback, session settings, repeated prepared queries,
  errors, cancellation, transaction visibility, and cleanup. Preserve fallback
  execution on the original ClientContext and transaction.
- Run compressed scans and cold JIT with toolkit headers, build caches, and shared
  build dependencies unavailable. Include process restart and repeat load/query.
- Run repository lint and relevant unit/integration tests. Complete the required
  three independent reviews before committing meaningful changes.

## Final architecture

After the first deliverable, separate the host-facing extension from the private
GPU engine using a versioned C boundary and owned Arrow data. Translate plans
through an explicit supported representation, initially evaluating Substrait;
keep native-scan transaction ownership and fallback semantics explicit.

Add a small loader that selects a bundled CUDA 12 or CUDA 13 backend based on
driver support. Each backend must satisfy the same runtime dependency contract.
Test selection failures and compatibility independently of SQL execution.

Move host integration to DuckDB's stable C extension API as the necessary hooks
become available. At the pinned v2 revision, transparent optimizer interception,
physical execution replacement, and native storage/MVCC access still use C++
interfaces; the C extension header also describes the API as not yet stable.
Resolve those upstream API gaps before claiming stable ABI support. Keep Sirius
out of the DuckDB source tree and pursue core-extension distribution only after
the ABI, loader, packaging, and cross-version tests meet that contract.

## Baseline evidence

The integrated build base passes repository pre-commit checks, four static bundle
tests, and three NVTX link tests. The complete shared engine and wrapper build on
DuckDB 1.5.6. The extension loads into vanilla DuckDB 1.5.6 Python and runs an
aggregate over a checkpointed native table with GPU execution enabled and
`enable_duckdb_fallback=false`.

These checks establish the starting point; they do not establish v2 compatibility
or a self-contained runtime artifact.

## V2 implementation and validation

The standalone shared engine, C++ test executable, combined static engine, and
independent wrapper now compile against the pinned v2 revision. The port covers
v2 typed identifiers and indexes, bound functions, expression-based table
filters, query-result retention, prepared EXECUTE wrappers, native block
suballocation, transaction visibility, Parquet metadata, and Substrait ingestion.
The Substrait compatibility patch applies to its generated build copy.

Prepared SELECT executions rebind to expose their current bound plan and
parameters to Sirius. Finalized EXECUTE plans also pass through the pinned-table
UPDATE guard, including executions whose target changed during rebinding. The
retained CPU fallback owns the original physical plan and executes in the same
ClientContext and transaction.

DATE-to-timestamp casts preserve DuckDB's infinity sentinels and conversion
bounds, including the intermediate microsecond bound for second/millisecond
targets. Overflow raises for `CAST` and produces NULL for `TRY_CAST`.

The focused C++ validation passed 622 test cases (1,162,313 assertions): 482 engine
and scan tests, 86 prepared-execution/lifecycle/MVCC/fallback tests, and 54 Parquet,
result-conversion, session-scope, and date-cast tests. The package tests also pass
four static-bundle and three NVTX-link cases. Repository pre-commit checks pass.

The independent host links only DuckDB core functions and Parquet. Sirius is
loaded from the separately built artifact. The smoke matrix passed 19 CPU/GPU
result comparisons with fallback disabled and confirmed 19 GPU executions. It
covers repeated SQL EXECUTE with different parameters, string MIN/MAX, native and
Parquet filters, joins, and aggregation. A compressed native scan also passed with
a fresh JIT cache. A private mount namespace hides the workspace
and `/usr/local`; the runtime contains the host, extension, configuration, and
ordinary C++ runtime libraries. Both the comparison matrix and the cold scan
passed there. The cold scan compiled two kernels with zero memory/disk cache hits
and returned SUM=24975000, COUNT=50000. A second process returned the same result
with zero compilations and two disk-cache hits.

The bundle has no RPATH/RUNPATH. Its ELF dependencies are `libcuda.so.1`,
`libnvidia-ml.so.1`, and ordinary glibc/libstdc++/libgcc libraries. It does not need
shared Sirius, DuckDB, RAPIDS, CUDA runtime, NVRTC, nvJitLink, NUMA, or OpenMP
libraries. Validation currently covers one Linux x86_64 CUDA 13 / SM120 GPU;
other architectures and CUDA 12 still need their own builds and execution tests.

### Remaining work

- V2 can absorb a cross-table arithmetic predicate into an arbitrary join
  condition. Sirius declines that plan and uses CPU fallback; translating this
  new plan shape is a follow-up for GPU coverage.
- Quent's existing NVTX injector can emit caught Rust thread-local-access panics
  during cuDF process teardown. This also occurs with the shared engine; the
  tested queries finish correctly and the process exits zero. Resolve the
  dependency's teardown ordering before treating the artifact as release-ready.
- Run the complete CI matrix, provider/network integration tests, multiple GPUs,
  and the external-data worker-pressure gate. Focused migration tests do not
  replace those checks. CI now checks the built host revision and runs the CLI
  comparison matrix instead of loading the v2 artifact into Python DuckDB 1.5.
  Provisioning Iceberg/Avro for this development revision still depends on matching
  external extension builds; those provider checks have not been validated.
- Implement the stable C boundary, CUDA 12/13 selection, and core-extension
  publication after the host API gaps above are resolved.

## Reproduce the bundled build

Initialize both pinned DuckDB checkouts and build/install the engine first:

```bash
git submodule update --init --recursive
pixi run -e vcpkg env VCPKG_CUDA_ARCHITECTURES=120 \
  cmake --preset vcpkg-release -DCMAKE_CUDA_ARCHITECTURES=120 \
  -DSIRIUS_BUILD_TESTS=OFF
pixi run -e vcpkg cmake --build build/vcpkg-release \
  --target sirius_library --parallel 12
pixi run -e vcpkg cmake --install build/vcpkg-release \
  --prefix "$PWD/build/v2-static-install" --component sirius_library
```

Configure the independent host and wrapper against the installed static package:

```bash
pixi run cmake -S sirius-duckdb/duckdb -B build/v2-host -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_STANDARD=20 \
  -DBUILD_UNITTESTS=OFF \
  '-DBUILD_EXTENSIONS=core_functions;parquet' \
  '-DSTATICALLY_LINK_EXTENSIONS=core_functions;parquet' \
  -DDUCKDB_EXTENSION_CONFIGS="$PWD/sirius-duckdb/extension_config.cmake" \
  -Dsirius_DIR="$PWD/build/v2-static-install/lib/cmake/sirius" \
  -DSIRIUS_DUCKDB_LINKAGE=static
pixi run cmake --build build/v2-host \
  --target duckdb sirius_loadable_extension --parallel 12
pixi run python test/scripts/v2_extension_smoke.py \
  --duckdb build/v2-host/duckdb \
  --extension build/v2-host/extension/sirius/sirius.duckdb_extension \
  --output build/v2-smoke
```

The smoke runner uses Python's standard library to drive the DuckDB CLI; it does
not import or build duckdb-python. Local artifacts are unsigned, so direct CLI
loads require `-unsigned`. The host and extension must use the same pinned v2
revision and compatible C++ toolchains.
