# Conda packages

[rattler-build](https://rattler.build) recipe for two packages built from this
checkout, for CUDA 12 and 13 (Linux only):

| Package | Contents |
| --- | --- |
| `libsirius` | Shared `libsirius.so`, C/C++ headers and the `sirius::sirius` CMake target. RAPIDS libraries are run dependencies. |
| `libsirius-static` | One self-contained `lib/sirius-static/lib/libsirius.a` with every dependency merged in, except the CUDA toolkit libraries. |

## Build

```sh
CONDA_OVERRIDE_CUDA=13 pixi exec --spec rattler-build -- rattler-build build \
  --config-file conda.recipe/rattler-build.toml \
  --recipe conda.recipe/recipe.yaml \
  --variant-config conda.recipe/variants.yaml \
  --variant cuda_version=13 \
  --output-dir output
```

Use `cuda_version=12` for the CUDA 12 packages. The static package builds RAPIDS
and the other dependencies from source with vcpkg, which takes hours; dependency
builds can be cached across runs by setting `VCPKG_BINARY_SOURCES` in the
environment, for example `clear;files,$HOME/.cache/vcpkg,readwrite`.

The recipe pins the glibc sysroot to 2.28, the floor shared with the RAPIDS
packages.

## Using `libsirius-static`

The package installs a prefix at `$PREFIX/lib/sirius-static`:

```cmake
project(app LANGUAGES CXX CUDA)
find_package(sirius CONFIG REQUIRED)  # -DCMAKE_PREFIX_PATH=$PREFIX/lib/sirius-static
target_link_libraries(app PRIVATE sirius::sirius_static)
```

- The consumer needs a CUDA toolkit with the same major version as the package
  (`cuda_version`); the config rejects any other.
- The consumer provides the CUDA toolkit static libraries (`cudart`, `cublas`,
  `cusolver`, `cusparse`, `curand`, `nvrtc`, `nvJitLink`), found through
  `CUDAToolkit`. Only `libculibos.a`, which conda's CUDA packages lack, ships
  next to `libsirius.a`.
- The UCX transport archives stay in `lib/ucx` and are linked whole, because
  they register themselves from constructors.
- Only the driver (`libcuda`), NVML and system libraries stay dynamic.
- The config records the DuckDB commit the archive was built from
  (`sirius_DUCKDB_REVISION`), so DuckDB extensions linked against it can verify
  they use the same revision.
