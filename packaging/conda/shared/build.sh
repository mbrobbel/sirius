#!/usr/bin/env bash
set -euo pipefail
export CARGO_NET_OFFLINE=true
export CARGO_HOME="$SRC_DIR/.cargo-home"
export CARGO_TARGET_DIR="$SRC_DIR/build-cargo"
cmake -S "$SRC_DIR" -B build-conda -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_PREFIX_PATH="$PREFIX;$BUILD_PREFIX" \
  -DCMAKE_CUDA_ARCHITECTURES="75;80;86;89;90" \
  -DCMAKE_CXX_COMPILER_LAUNCHER= \
  -DCMAKE_CUDA_COMPILER_LAUNCHER= \
  -DSIRIUS_BUILD_SHARED=ON -DSIRIUS_BUILD_STATIC=OFF \
  -DSIRIUS_BUILD_TESTS=OFF -DSIRIUS_BUILD_S3_TESTS=OFF
cmake --build build-conda --target sirius_shared --parallel "$CPU_COUNT"
cmake --install build-conda --component sirius_library
python "$SRC_DIR/packaging/collect-licenses.py" "$SRC_DIR" "$PREFIX/share/licenses/sirius"
