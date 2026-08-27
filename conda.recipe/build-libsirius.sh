#!/usr/bin/env bash
set -euo pipefail

case "${cuda_version}" in
  12) cuda_archs="75-real;80-real;86-real;89-real;90a-real" ;;
  13) cuda_archs="75-real;80-real;86-real;89-real;90a-real;100f-real;120a-real;120" ;;
  *) echo "unsupported CUDA version: ${cuda_version}" >&2; exit 1 ;;
esac

test -f "${SRC_DIR}/duckdb/CMakeLists.txt"
test -f "${SRC_DIR}/cucascade/CMakeLists.txt"
test -f "${SRC_DIR}/substrait/CMakeLists.txt"

cmake -S "${SRC_DIR}/duckdb" -B build-libsirius -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_CUDA_ARCHITECTURES="${cuda_archs}" \
  -DCMAKE_INSTALL_PREFIX="${PREFIX}" \
  -DCMAKE_INSTALL_RPATH='$ORIGIN' \
  -DDUCKDB_EXTENSION_CONFIGS="${SRC_DIR}/extension_config.cmake" \
  -DEXPORT_DYNAMIC_SYMBOLS=ON \
  -DEXTENSION_STATIC_BUILD=ON \
  -DOVERRIDE_GIT_DESCRIBE=v1.5.5 \
  -DSIRIUS_BUILD_S3_TESTS=OFF \
  -DSIRIUS_BUILD_SHARED_LIBRARY=ON \
  -DSIRIUS_EXTENSION_VERSION="${PKG_VERSION}" \
  -DSIRIUS_PACKAGE_VERSION="${PKG_VERSION}"

cmake --build build-libsirius --target sirius_shared -j"${CPU_COUNT}"
cmake --install build-libsirius --component SiriusLibrary
