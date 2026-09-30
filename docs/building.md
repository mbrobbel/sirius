# Building Sirius

## Separate library and extension builds

The repository root is the Sirius CMake project. It owns the engine libraries;
DuckDB is a temporary source dependency behind the provider described below.
No DuckDB extension helper creates a Sirius library target.

| Target | Output | Dependencies |
| --- | --- | --- |
| `sirius_shared` | `libsirius.so` | Shared dependencies from the Conda environment |
| `sirius_static` | `libsirius.a` | Bundled static dependencies from vcpkg and the CUDA toolkit |

The static package retains only platform and NVIDIA driver runtime dependencies.
The Conda and vcpkg presets use separate build directories and dependency sets.

```bash
pixi run cmake -S . -B build/sirius -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_CUDA_ARCHITECTURES=75 \
  -DSIRIUS_BUILD_S3_TESTS=OFF
pixi run cmake --build build/sirius --target sirius_shared
pixi run cmake --install build/sirius --prefix "$PWD/build/install" --component sirius_library

pixi run cmake -S duckdb -B build/sirius-duckdb -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DOVERRIDE_GIT_DESCRIBE=v1.5.6 \
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
not compile engine, CUDA, or Rust sources. The shared wrapper requires the
installed Sirius library and its runtime dependencies.

## Static distribution builds

The vcpkg and `ci-release` presets build a combined `libsirius.a` containing the
engine and its static dependencies. The separate extension build consumes it
with `find_package(sirius CONFIG REQUIRED COMPONENTS static)` and
`sirius::sirius_static`. Select this mode with `SIRIUS_DUCKDB_LINKAGE=static`.
No engine sources are compiled by the extension build.

The vcpkg ports build cuVS and RAFT with OpenMP disabled, so the extension
does not require `libgomp.so`.

RAPIDS dependencies default to their full supported GPU architecture set. To
shorten a local static build, select architectures for both the dependencies and
Sirius itself, for example:

```bash
pixi run -e vcpkg env VCPKG_CUDA_ARCHITECTURES=120 \
  cmake --preset vcpkg-release -DCMAKE_CUDA_ARCHITECTURES=120
```

`VCPKG_CUDA_ARCHITECTURES` accepts the same semicolon-separated architecture list
as CMake (quote lists in the shell), or `RAPIDS`. Keep the dependency set at least
as broad as Sirius's `CMAKE_CUDA_ARCHITECTURES`; an SM120-only package requires an
SM120 GPU. The raw environment value participates in the triplet's binary cache
key for every port, so changing it rebuilds dependencies. Use one spelling
consistently: an unset override and explicit `RAPIDS` have separate cache keys.

`make ci-release` stages the bundled extension at
`build/ci-release/extension/sirius/sirius.duckdb_extension`, matching the
distribution workflow. The artifact check rejects runtime search paths and
shared dependencies other than platform libraries and the NVIDIA driver.

For direct CMake builds, enable `SIRIUS_BUILD_STATIC` with static dependencies,
build `sirius_static`, and install the `sirius_library` component.
`SIRIUS_BUILD_SHARED=OFF` skips the shared library. The installed static package
uses the CUDA toolkit's driver stubs for host linking; it does not compile CUDA.

## Shared implementation objects

The internal `sirius_objects` CMake target compiles the common C++ and CUDA
implementation once per build configuration. Its objects form `sirius_core`, an
internal archive, and the shared Sirius library. The DuckDB extension links the
installed shared library. CUDA device linking takes place on concrete library targets, not on the
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

The intended replacement is `find_package(duckdb CONFIG REQUIRED)` using a conda
package with the required headers and libraries. Decoupling the library's and
extension's DuckDB versions requires a separate API/ABI change.

## Installed CMake package

Install the `sirius_library` component and consume `sirius::sirius` with
`find_package(sirius CONFIG REQUIRED)`. Its public headers do not require CUDA or
DuckDB headers. The shared library records its runtime dependencies.

```bash
pixi run cmake --install build/release --prefix "$PWD/build/stage" --component sirius_library
mv build/stage build/relocated
pixi run cmake -S test/cmake/installed_consumer -B build/consumer \
  -DCMAKE_PREFIX_PATH="$PWD/build/relocated"
pixi run cmake --build build/consumer
```

Consumers are responsible for DuckDB and C++ runtime ABI compatibility.
