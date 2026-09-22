#!/usr/bin/env bash
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
set -euo pipefail
phase="${1:?build or gpu}"
channel="$(realpath "${2:?local channel}")"
linkage="${3:?shared or static}"
cuda="${4:?CUDA version}"
case "$phase:$linkage" in
  build:shared|build:static|gpu:shared|gpu:static) ;;
  *) echo 'Expected build|gpu and shared|static' >&2; exit 1 ;;
esac
root="$(pwd)"
prefix="$root/build/package-consumer-$linkage"
build="$root/build/package-wrapper-$linkage"
package=libsirius
if [[ "$linkage" == static ]]; then package=libsirius-static; fi
platform=linux-64
if [[ "$(uname -m)" == aarch64 ]]; then platform=linux-aarch64; fi
conda index "$channel"
conda create -y -p "$prefix" --override-channels -c "file://$channel" -c rapidsai -c conda-forge \
  "$package=0.0.0=cuda${cuda%%.*}_*" 'libsirius-devel=0.0.0' \
  "gcc_${platform}=14" "gxx_${platform}=14" 'cmake>=3.30.4' ninja 'python>=3.12' \
  'rust>=1.88' cuda-driver-dev cuda-nvml-dev "binutils_${platform}"
eval "$(conda shell.bash hook)"
conda activate "$prefix"
unset RUSTC_WRAPPER
export SIRIUS_PREFIX="$prefix"
export LD_LIBRARY_PATH="$prefix/lib:${LD_LIBRARY_PATH:-}"
if [[ "$linkage" == static ]]; then
  test ! -e "$prefix/lib/libsirius.so"
  test ! -e "$prefix/lib/libcudf.so"
fi
if [[ "$phase" == build ]]; then
  cmake -S test/cmake/installed_consumer -B "$build/consumer" -G Ninja \
    -DCMAKE_PREFIX_PATH="$prefix" -DSIRIUS_TEST_LINKAGE="$linkage"
  cmake --build "$build/consumer"
  features=()
  if [[ "$linkage" == static ]]; then features=(--features "sirius/static,sirius-sys/static"); fi
  CARGO_TARGET_DIR="$build/rust" cargo test --locked --no-run --manifest-path rust/Cargo.toml \
    -p sirius -p sirius-sys "${features[@]}"
  # Leave Sirius out of the host so the GPU job exercises loading the artifact.
  cat > "$build/extensions.cmake" <<CMAKE
 duckdb_extension_load(sirius DONT_LINK SOURCE_DIR "$root/sirius-duckdb"
   INCLUDE_DIR "$root/sirius-duckdb/src/include" EXTENSION_VERSION dev)
CMAKE
  revision="$(git -C duckdb rev-parse --short=10 HEAD)"
  cmake -S duckdb -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_PREFIX_PATH="$prefix" -DSIRIUS_DUCKDB_LINKAGE="$linkage" \
    -DDUCKDB_EXTENSION_CONFIGS="$build/extensions.cmake" \
    -DOVERRIDE_GIT_DESCRIBE="v1.5.5-0-g$revision" \
    '-DBUILD_EXTENSIONS=core_functions;parquet' -DBUILD_UNITTESTS=OFF \
    -DENABLE_SANITIZER=OFF -DENABLE_UBSAN=OFF -DCMAKE_EXPORT_COMPILE_COMMANDS=ON \
    -DCMAKE_CXX_COMPILER_LAUNCHER= -DCMAKE_C_COMPILER_LAUNCHER= \
    -DCMAKE_CXX_SCAN_FOR_MODULES=OFF
  cmake --build "$build" --target shell sirius_loadable_extension --parallel "$(nproc)"
  python packaging/check-artifact.py "$build/extension/sirius/sirius.duckdb_extension" "$linkage"
else
  export SIRIUS_CONFIG_FILE="$root/test/cpp/integration/integration.yaml"
  extension="$build/extension/sirius/sirius.duckdb_extension"
  result="$("$build/duckdb" -unsigned -csv -noheader -c "
    LOAD '$extension';
    SET gpu_execution = false;
    CREATE TABLE package_smoke AS SELECT i::BIGINT AS i FROM range(10000) t(i);
    SET enable_duckdb_fallback = false;
    SET gpu_execution = true;
    SELECT sum(i)::BIGINT FROM package_smoke;
    SELECT * FROM gpu_execution('SELECT sum(i)::BIGINT FROM package_smoke');")"
  test "$result" = $'49995000\n49995000'
fi
