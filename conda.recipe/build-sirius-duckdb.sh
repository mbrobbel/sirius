#!/usr/bin/env bash
set -euo pipefail

cmake -S "${SRC_DIR}/duckdb" -B build-sirius-duckdb -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="${PREFIX}" \
  -DCMAKE_PREFIX_PATH="${PREFIX}" \
  -DDUCKDB_EXTENSION_CONFIGS="${SRC_DIR}/extension_config.cmake" \
  -DEXTENSION_STATIC_BUILD=OFF \
  -DOVERRIDE_GIT_DESCRIBE=v1.5.5 \
  -DSIRIUS_BUILD_SHARED_LIBRARY=OFF \
  -DSIRIUS_EXTENSION_FROM_BUNDLE=ON \
  -DSIRIUS_EXTENSION_VERSION="${PKG_VERSION}" \
  -DSIRIUS_PACKAGE_VERSION="${PKG_VERSION}"

cmake --build build-sirius-duckdb --target sirius_loadable_extension -j"${CPU_COUNT}"
cmake --install build-sirius-duckdb --component SiriusDuckDB
patchelf --remove-rpath "${PREFIX}/lib/sirius.duckdb_extension"

mkdir -p "${PREFIX}/etc/conda/activate.d" "${PREFIX}/etc/conda/deactivate.d"
cp "${RECIPE_DIR}/activate-sirius-duckdb.sh" \
  "${PREFIX}/etc/conda/activate.d/sirius-duckdb.sh"
cp "${RECIPE_DIR}/deactivate-sirius-duckdb.sh" \
  "${PREFIX}/etc/conda/deactivate.d/sirius-duckdb.sh"
