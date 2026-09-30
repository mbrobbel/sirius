# Sirius conda packages

The recipe produces three co-installable packages:

| Package | Contents | CMake target |
| --- | --- | --- |
| `sirius` | Shared library; runtime dependencies from conda | `sirius::sirius` |
| `sirius-static` | Bundled archive, including its static dependencies | `sirius::sirius_static` |
| `sirius-devel` | Common public headers and CMake config | — |

Both libraries depend on `sirius-devel`. The static package uses the repository's
vcpkg ports; its consumers need only NVIDIA driver and base OS libraries at
runtime. The shared and static libraries use separate dependency configurations.
Both are built against the pinned DuckDB revision, which C++ extension consumers
must also use.

Initialize the build dependencies, then build the packages:

```bash
git submodule update --init duckdb substrait cucascade
export SIRIUS_BUILD_ID="$(date -u +%Y%m%d%H%M%S%N)"
pixi exec --spec rattler-build=0.76.1 -- rattler-build build \
  --recipe conda.recipe --channel rapidsai --channel conda-forge \
  --output-dir build/conda --no-build-id --env-isolation none
```

Use `--up-to sirius` or `--up-to sirius-static` to build one library and its
common development package. Building `sirius-devel` does not compile the engine.

The recipe supports Linux amd64 (`linux-64`) and arm64 (`linux-aarch64`), with
CUDA 12.9 / GCC 14 and CUDA 13.4 / GCC 15. `variants.yaml` selects the shared
build's RAPIDS release and pairs each CUDA version with its compiler and GPU
architectures. Static RAPIDS versions are pinned by the vcpkg ports. CI builds
and tests both libraries for all four combinations on native runners using
[`prefix-dev/rattler-build-action`](https://github.com/prefix-dev/rattler-build-action).
Package tests compile installed CMake consumers; the static test also checks
that no additional shared runtime dependencies were introduced.

Use a fresh `SIRIUS_BUILD_ID` for each standalone rattler-build invocation,
including uncommitted changes, so binary consumers cannot reuse a cached package
from another build.

Both builds use sccache for C, C++, CUDA, and Rust. `--no-build-id` keeps the
build directories stable; it does not change `SIRIUS_BUILD_ID`.
`--env-isolation none` preserves the runner's cache configuration. CI uses the
same sccache and vcpkg cache backends and `sirius` prefix as the other builds.

Pixi consumers can enable `preview = ["pixi-build"]` and use this directory as a
source dependency for `sirius` or `sirius-static`, with `build-variants-files`
pointing to `variants.yaml`. Pixi resolves `sirius-devel` from the same recipe,
tracks source changes, and caches built packages. These builds use the default
`source` build ID. The extension directory is excluded from the library sources.

`build/conda` is a local channel containing all outputs. CI can transfer its
packages without publishing them; the extension build should consume the static
package and matching development package from the same checkout being tested.

`SIRIUS_CMAKE_ARGS` can supply additional CMake options, for example local
`FETCHCONTENT_SOURCE_DIR_*` download caches or an existing `VCPKG_INSTALLED_DIR`.
