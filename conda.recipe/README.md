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
  --recipe conda.recipe --channel rapidsai --channel conda-forge \
  --output-dir build/conda --no-build-id --env-isolation none
```

The recipe supports Linux amd64 (`linux-64`) and arm64 (`linux-aarch64`), with
CUDA 12.9 / GCC 14 and CUDA 13.4 / GCC 15. `variants.yaml` selects the RAPIDS release
and pairs each CUDA version with its compiler and GPU architectures. CI builds
and tests all four combinations on native runners using
[`prefix-dev/rattler-build-action`](https://github.com/prefix-dev/rattler-build-action).

A local build produces both CUDA variants for the native CPU architecture.
The recipe builds only the
shared library and installs the `sirius_library` component. Package tests check
the files and compile an independent CMake consumer without initializing a GPU.

Use a fresh `SIRIUS_BUILD_ID` for each standalone rattler-build invocation,
including uncommitted changes, so binary consumers cannot reuse a cached package
from another build.

The recipe uses sccache for C, C++, CUDA, and Rust. `--no-build-id` keeps the
build directory stable for cache reuse; it does not change `SIRIUS_BUILD_ID`.
`--env-isolation none` preserves the runner's cache configuration. CI uses the
same cache backend and `sirius` prefix as the other builds.

Pixi consumers can enable `preview = ["pixi-build"]` and use this directory as a
source dependency, with `build-variants-files` pointing to `variants.yaml`. Pixi
tracks source changes and caches the built package; these builds use the default
`source` build ID. The extension directory is excluded from the library sources.

`build/conda` is a local channel. A downstream Pixi environment can resolve
`sirius` from it alongside `rapidsai` and `conda-forge`; CI can transfer the channel
as an artifact without publishing packages. The extension build should consume
the package from the same checkout being tested.

`SIRIUS_CMAKE_ARGS` can supply additional CMake options, for example local
`FETCHCONTENT_SOURCE_DIR_*` download caches.
