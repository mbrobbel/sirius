#!/usr/bin/env bash
set -euo pipefail
export LIBCLANG_PATH="$BUILD_PREFIX/lib"

cmake -S duckdb -B build-conda -G Ninja \
  ${CMAKE_ARGS:-} ${SIRIUS_CMAKE_ARGS:-} \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_PREFIX_PATH="$PREFIX;$BUILD_PREFIX" \
  -DCMAKE_CUDA_ARCHITECTURES="75;80;86;89;90;100;120" \
  -DCMAKE_CXX_SCAN_FOR_MODULES=OFF \
  -DOVERRIDE_GIT_DESCRIBE=v1.5.6 \
  -DENABLE_SANITIZER=OFF -DENABLE_UBSAN=OFF \
  -DBUILD_UNITTESTS=OFF -DBUILD_SHELL=OFF \
  -DSIRIUS_BUILD_S3_TESTS=OFF \
  -DDUCKDB_EXTENSION_CONFIGS="$SRC_DIR/extension_config.cmake"
cmake --build build-conda --target sirius_shared --parallel "$CPU_COUNT"
cmake --install build-conda --component sirius_library
