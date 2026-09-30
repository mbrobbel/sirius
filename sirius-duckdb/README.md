# Sirius DuckDB extension

This directory contains the out-of-tree DuckDB wrapper. It consumes the installed
Sirius CMake package; the engine is built separately at the repository root.

The wrapper was imported from [sirius-db/sirius-duckdb](https://github.com/sirius-db/sirius-duckdb)
at `ca1297c79bb4378f2a19464c5a7e61214e2812d9`, with its Git history. Its source layout,
development environment, SQL smoke test, extension tools, and DuckDB v2 checkout
are retained. The engine registration and shared/static package consumer come
from the Sirius build split. Distribution workflows remain at the repository
root, where the engine and wrapper can be built together.
Formatting configuration links point to the parent repository so linting also
works without initializing the wrapper's DuckDB checkout.

## Build

Initialize the wrapper's host and extension tools from the repository root:

```bash
git submodule update --init sirius-duckdb/duckdb sirius-duckdb/extension-ci-tools
```

Build and install the engine first, then configure the wrapper through DuckDB.
The following commands use an engine installed at `build/release/install`:

```bash
pixi run cmake -S duckdb -B build/wrapper -G Ninja \
  -DDUCKDB_EXTENSION_CONFIGS="$PWD/sirius-duckdb/extension_config.cmake" \
  -Dsirius_DIR="$PWD/build/release/install/lib/cmake/sirius" \
  -DSIRIUS_DUCKDB_LINKAGE=shared \
  -DBUILD_SHELL=ON -DBUILD_UNITTESTS=ON
pixi run cmake --build build/wrapper --target duckdb sirius_loadable_extension
```

The output is `build/wrapper/extension/sirius/sirius.duckdb_extension`. For an
installed static engine bundle, select `-DSIRIUS_DUCKDB_LINKAGE=static` instead.
That mode checks the extension's runtime dependencies after linking. Until the
v2 engine port is complete, both linkage modes use the root DuckDB checkout;
the imported v2 build helpers also need adaptation for the static bundle.

The wrapper's Makefile also supports the standard DuckDB extension tools. Pass
the installed package location and linkage through `EXT_FLAGS`.

## Compatibility

This wrapper currently uses DuckDB's **C++ extension API**. The engine, wrapper,
and loading host must use the same DuckDB revision and compatible compiler ABI.
The imported wrapper pins a v2 host; the engine's v2 port must be completed before
that host can consume the engine. During the port, the root build can still use
the root `duckdb/` checkout for its matching wrapper build.

A successful build does not establish stable C API compatibility. Separating
the host and private DuckDB instances, adding the CUDA 12/13 selector, and using
the stable extension API are later migration stages.
