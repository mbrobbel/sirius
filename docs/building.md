# Building Sirius

## Separate DuckDB extension

[`sirius-duckdb/`](../sirius-duckdb/README.md) contains the independent extension
setup, with its own DuckDB checkout, Pixi environment, and Makefile. It consumes
the [static Sirius package](../conda.recipe/README.md) and runs GPU SQL tests
through the normal DuckDB extension test target. The root build continues to
provide the existing extension and static distribution build.

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

## Bundled static library

The vcpkg configuration can also produce `libsirius.a`, including its static
DuckDB, RAPIDS, CUDA toolkit, and host library dependencies. It reuses the engine
objects from that configuration. The archive rejects unexpected shared library
dependencies; the CUDA driver and standard platform libraries remain external.

```bash
pixi run -e vcpkg cmake -S duckdb --preset vcpkg-release -DSIRIUS_BUILD_STATIC=ON
pixi run -e vcpkg cmake --build build/vcpkg-release --target sirius_static
pixi run -e vcpkg cmake --install build/vcpkg-release \
  --component sirius_static_library --prefix "$PWD/build/static-install"
```

Consumers use `find_package(sirius CONFIG REQUIRED COMPONENTS static)` and link
`sirius::sirius_static`. The installed target carries the linker requirements;
the consumer needs a CUDA toolkit at build time for the driver stubs. Shared
builds continue to use `sirius::sirius`.

The static target extracts the compiled NVTX object from `libsirius.a` into the
consumer's build directory and links it directly, preserving its public entry
point when a consumer hides archive symbols. No additional object is shipped or
compiled.
