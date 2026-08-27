# Sirius conda packages

The recipe produces `libsirius`, `libsirius-static`, and `sirius-duckdb` for
CUDA 12 and CUDA 13. Builds are native Linux builds; run them on x86_64 and
aarch64 hosts to cover both supported architectures.

```bash
CONDA_OVERRIDE_CUDA=13 \
pixi exec --spec rattler-build=0.75.0 -- \
  rattler-build build \
  --config-file conda.recipe/rattler-build.toml \
  --recipe conda.recipe/recipe.yaml
```

The static output uses the repository's vcpkg manifest and overlay ports. Set
`VCPKG_BINARY_SOURCES` before the build to reuse an existing binary cache. The
first uncached build downloads the pinned vcpkg, CMake FetchContent, and Cargo
sources and can take substantially longer than the shared-library build.
The package installs a whole-linked Sirius core archive and a selectively
linked dependency archive so static third-party runtimes remain usable from a
shared DuckDB extension.

`sirius-duckdb` links the exact CUDA-matched `libsirius-static` output during
the build, but the installed extension has no runtime dependency on that
package. Its activation script exposes the absolute extension path as
`SIRIUS_EXTENSION`.
