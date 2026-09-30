# Shared Sirius package

The `sirius` package contains `libsirius.so`, public headers, and the installed
CMake package. Its runtime dependencies come from conda. It is built against the
DuckDB revision pinned by this repository; C++ extension consumers must use the
same revision.

Initialize the build dependencies, then build the package:

```bash
git submodule update --init duckdb substrait cucascade
export SIRIUS_BUILD_ID="$(date -u +%Y%m%d%H%M%S%N)"
pixi exec --spec rattler-build=0.76.1 -- rattler-build build \
  --recipe packaging/conda --channel rapidsai --channel conda-forge \
  --output-dir build/conda
```

The initial variant is Linux with CUDA 13.4 and GCC 15. The recipe builds only the
shared library and installs the `sirius_library` component. Package tests check
the files and compile an independent CMake consumer without initializing a GPU.

Use a fresh `SIRIUS_BUILD_ID` for each local build, including uncommitted changes.
CI uses the source revision and workflow run to give each package a distinct
identity, so consumers cannot reuse a cached package from another build.

`build/conda` is a local channel. A downstream Pixi environment can resolve
`sirius` from it alongside `rapidsai` and `conda-forge`; CI can transfer the channel
as an artifact without publishing packages. The extension build should consume
the package from the same checkout being tested.

`SIRIUS_CMAKE_ARGS` can supply additional CMake options, for example local
`FETCHCONTENT_SOURCE_DIR_*` download caches.
