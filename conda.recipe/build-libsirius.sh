#!/usr/bin/env bash
set -euo pipefail

source "${RECIPE_DIR}/cuda-archs.sh"
source "${RECIPE_DIR}/source-git.sh" duckdb

# bindgen (nvtx-sys in the telemetry bridge) needs libclang and a clang binary.
export LIBCLANG_PATH="${BUILD_PREFIX}/lib"
export CLANG_PATH="${BUILD_PREFIX}/bin/clang"

# DuckDB cannot read the commit from the synthetic checkout.
export DUCKDB_COMMIT="${duckdb_revision}"

cmake -S "${SRC_DIR}" -B build -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_CUDA_ARCHITECTURES="${cuda_archs}" \
  -DCMAKE_INSTALL_PREFIX="${PREFIX}" \
  -DCMAKE_PREFIX_PATH="${PREFIX}" \
  -DCMAKE_CXX_SCAN_FOR_MODULES=OFF \
  -DSIRIUS_BUILD_SHARED=ON \
  -DSIRIUS_BUILD_STATIC=OFF \
  -DSIRIUS_BUILD_TESTS=OFF \
  -DSIRIUS_DISTRIBUTION_DEFAULTS=ON

cmake --build build --target sirius_library -j"${CPU_COUNT}"
cmake --install build --component sirius_library
