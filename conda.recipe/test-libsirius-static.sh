#!/usr/bin/env bash
set -euo pipefail

root="${PREFIX}/lib/sirius-static"
test -s "${root}/lib/libsirius.a"
test -f "${root}/share/sirius/sirius-config.cmake"
test -f "${root}/include/sirius-c/version.h"
test -f "${root}/share/sirius/sirius-cuda-fatbin.ld"
grep -q 'sirius_DUCKDB_REVISION "[0-9a-f]\{40\}"' "${root}/share/sirius/sirius-config.cmake"

# Everything but the CUDA toolkit and the UCX transports is merged into the one archive.
test "$(ls "${root}/lib" | tr '\n' ' ')" = "libculibos.a libsirius.a ucx "
test "$(ls "${root}/lib/ucx" | tr '\n' ' ')" = "libucm_cuda.a libuct_cma.a libuct_cuda.a "

cmake -S tests/static-consumer -B tests/static-consumer/build -G Ninja \
  -DCMAKE_PREFIX_PATH="${root}" \
  -DCMAKE_CUDA_ARCHITECTURES=80
cmake --build tests/static-consumer/build
tests/static-consumer/build/sirius_static_consumer
