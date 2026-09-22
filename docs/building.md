# Building Sirius

## Separate library and extension builds

The repository root is the Sirius CMake project. It owns the engine libraries;
DuckDB is a temporary source dependency behind the provider described below.
No DuckDB extension helper creates a Sirius library target.

```bash
pixi run cmake -S . -B build/sirius -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_CUDA_ARCHITECTURES=75 \
  -DSIRIUS_BUILD_S3_TESTS=OFF
pixi run cmake --build build/sirius --target sirius_shared
pixi run cmake --install build/sirius --prefix "$PWD/build/install" --component sirius_library

pixi run cmake -S duckdb -B build/sirius-duckdb -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DOVERRIDE_GIT_DESCRIBE=v1.5.5 \
  -DEXTENSION_STATIC_BUILD=ON \
  -DDUCKDB_EXTENSION_CONFIGS="$PWD/sirius-duckdb/extension_config.cmake" \
  -Dsirius_DIR="$PWD/build/install/lib/cmake/sirius"
pixi run cmake --build build/sirius-duckdb --target sirius_loadable_extension
```

`pixi run make` drives the same sequence with the release preset: engine and C++
tests in `build/release`, installed package in `build/release/install`, and DuckDB
in `build/release/sirius-duckdb`. Existing build directories configured with
DuckDB as the root must be removed before using the new root presets.

`sirius-duckdb/` contains only extension entrypoints and packaging metadata. It
uses the installed Sirius package and DuckDB's extension build helpers. It does
not compile engine, CUDA, or Rust sources.

## Shared implementation objects

The internal `sirius_objects` CMake target compiles the common C++ and CUDA
implementation once per build configuration. Its objects form `sirius_core`, an
internal archive, and feed the shared Sirius library and DuckDB extension
outputs directly. CUDA device linking takes place on concrete library targets, not on the
object target.

Compile options, dependency headers, PIC, and visibility belong to the object
target. Final library targets also declare their link dependencies: consuming
`$<TARGET_OBJECTS:sirius_objects>` alone does not propagate usage requirements.
Objects are shared only within compatible compiler and dependency configurations.

DuckDB entrypoints are separate from the engine's registration API. NVTX setup is
part of the shared implementation objects. At runtime the ELF loader's link map
identifies whether the code is embedded in the main executable (including PIE)
or a shared library. Libraries publish their own image path; executables use the
private injection sentinel and exported initializer. No filename convention or
output-specific compilation is required.

## NVTX linkage tests

These tests need a C++ compiler but no GPU, CUDA toolkit, or DuckDB build. They
check environment precedence, explicit injector configuration, discovery in PIE and non-PIE executables and shared libraries, and forwarding to the embedded initializer.

```bash
pixi run cmake -S test/cmake/nvtx_injection -B build/nvtx-test -G Ninja
pixi run cmake --build build/nvtx-test
pixi run ctest --test-dir build/nvtx-test --output-on-failure
```

## DuckDB dependency

`cmake/sirius-duckdb-provider.cmake` owns the temporary source dependency. Its
`sirius::duckdb_dependency` target carries DuckDB headers, compile definitions,
and the core and Parquet libraries. `SIRIUS_DUCKDB_SOURCE_DIR` selects the source
tree; use the revision pinned by this repository. This is a build-only contract,
not an installed Sirius target or a stable DuckDB ABI.

## Installed CMake package

Install the `sirius_library` component and consume `sirius::sirius` with
`find_package(sirius CONFIG REQUIRED)`. Its public headers do not require CUDA or
DuckDB headers. The shared library records its runtime dependencies; an installed
consumer does not need the engine's CMake dependency targets.

```bash
pixi run cmake --install build/release --prefix "$PWD/build/stage" --component sirius_library
mv build/stage build/relocated
pixi run cmake -S test/cmake/installed_consumer -B build/consumer \
  -DCMAKE_PREFIX_PATH="$PWD/build/relocated"
pixi run cmake --build build/consumer
```

The package records the DuckDB revision, compiler, and build mode. The extension
wrapper must call `sirius_check_duckdb_compatibility` before linking to that C++
API; independent DuckDB versions are not supported yet.

## Tests

Enable `SIRIUS_BUILD_TESTS` for the engine's C++ tests. Their test-only loader
registers the engine directly, preserving automatic registration without linking
the DuckDB wrapper. `SIRIUS_BUILD_S3_TESTS` only adds its Go/container harness when
engine tests are enabled.

```bash
pixi run ctest --test-dir build/release -L gpu --output-on-failure
pixi run ctest --test-dir build/release -R nvtx --output-on-failure
```

SQLLogic tests belong to `sirius-duckdb/test/sql` and run with DuckDB's test
runner from the separate wrapper build. Run them from the repository root so
fixture paths resolve. GPU tests require an NVIDIA device and driver.

## Combined static archive

`SIRIUS_BUILD_STATIC=ON` creates `libsirius.a`; `SIRIUS_BUILD_SHARED=OFF` skips the
shared output. Build target `sirius_static`, then install the same
`sirius_library` component. Use the vcpkg static dependency environment for this
mode. A shared redistributable dependency makes archive assembly fail.

The archive includes Sirius's resolved CUDA device link and its redistributable
static dependencies, including DuckDB and the Rust bridge. CMake dependency
interfaces determine membership. The installed `sirius::sirius_static` target
retains registration objects with whole-archive linking and declares the
remaining platform, C++ runtime, and NVIDIA driver libraries. It does not require
vcpkg or internal dependency targets in a consumer project.

The build writes `libsirius.a.json` with input names and SHA-256 hashes, and
`libsirius.a.cmake` with the external link requirements. Build paths are excluded
from installed metadata. A static-only install also exposes `sirius::sirius`.

The archive helper can be tested without CUDA:

```bash
pixi run cmake -S test/cmake/static_bundle -B build/archive-test -G Ninja
pixi run cmake --build build/archive-test
pixi run ctest --test-dir build/archive-test --output-on-failure
```
