# DuckDB v2 migration

The engine and wrapper use unmodified DuckDB revision
`561522aea03e400bd20adc64fcc63e78b8721f3f`. They still use DuckDB's C++ APIs and
must agree on that revision and a compatible toolchain. This is not yet a stable
C ABI boundary.

## Branch order

1. `build/v2/01-standalone-root`: support standalone Sirius CMake while retaining
   the integrated extension build during the transition.
2. `build/v2/02-static-package`: install normal static libraries and exported
   CMake targets, with third-party dependencies supplied separately.
3. `build/v2/03-duckdb-wrapper`: build the wrapper against shared Sirius for local
   development and through its Sirius vcpkg port for static distribution. Switch
   root Make and CI to the separate builds and remove the integrated path.
4. `build/v2/04-engine-port`: move both DuckDB pins to v2 and adapt the engine's
   identifiers, plans, results, registration, native scans and Substrait reader.
5. `build/v2/05-extension-validation`: compare supported GPU execution with the
   matching independent CPU host in shared and bundled-extension CI.

Conda packaging is independent of this sequence. The wrapper remains in this
repository until the repository move is approved. Upstream extension-ci-tools
workflow integration follows the v2 migration; only its normal Makefile is used
by the direct distribution build.

## Build and test

Follow [the wrapper build instructions](../sirius-duckdb/README.md). The distribution
build uses CUDA 13 by default; replace `-e vcpkg` with `-e vcpkg-cuda12` for CUDA 12.
Use separate build directories when changing toolchains. For a smaller local
build, set both `CUDAARCHS` and `VCPKG_CUDA_ARCHITECTURES` to the target GPU's
architecture. The latter participates in the vcpkg binary cache key.

The distribution workflow builds amd64 and arm64 artifacts for both CUDA versions.
The loadable extension's post-link check rejects shared engine, RAPIDS, CUDA
runtime/JIT and other unexpected dependencies, and removes runtime search paths.
The allowed runtime dependencies are NVIDIA driver libraries and ordinary
Linux runtime libraries. Compiler runtimes are linked statically.

On a GPU machine, compare the separately built host and extension:

```sh
SIRIUS_CONFIG_FILE="$PWD/sirius-duckdb/test/config.yaml" pixi run bash -c \
  'export LD_LIBRARY_PATH="$CONDA_PREFIX/lib:${LD_LIBRARY_PATH:-}"; exec "$@"' -- \
  python test/scripts/v2_extension_smoke.py \
  --duckdb sirius-duckdb/build/release/duckdb \
  --extension sirius-duckdb/build/release/extension/sirius/sirius.duckdb_extension \
  --output build/v2-smoke
```

The smoke test disables fallback, compares CPU/GPU results, and checks GPU
execution logs. It covers native and Parquet scans, filters, aggregation, joins,
prepared statements and casts. C++ tests additionally cover transactions,
lifecycle and fallback. GPU CI currently exercises the bundled amd64 CUDA 13
artifact; CUDA 12 and arm64 still need runtime coverage.

## Remaining work

- Complete CUDA 12, arm64, multi-GPU and clean-runtime validation.
- Supply matching Iceberg/Avro extension builds for the pinned development host.
  Provider provisioning remains a required CI step and fails if they are absent.
- Investigate the previously observed Quent NVTX teardown warnings and extend
  cold-JIT, multi-GPU and clean-runtime validation.
- Move the wrapper repository when approved, then complete upstream
  extension-ci-tools integration without changing the CMake package contract.
- A stable C boundary and cross-version support require host hooks for optimizer
  interception, execution replacement and native-storage transaction access.
