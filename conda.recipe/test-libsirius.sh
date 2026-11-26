#!/usr/bin/env bash
set -euo pipefail

test -f "${PREFIX}/include/sirius-c/version.h"
test -f "${PREFIX}/lib/cmake/sirius/sirius-config.cmake"
test -f "${PREFIX}/lib/libsirius.so"
grep -q 'sirius_DUCKDB_REVISION "[0-9a-f]\{40\}"' "${PREFIX}/lib/cmake/sirius/sirius-config.cmake"
test "$(patchelf --print-rpath "${PREFIX}/lib/libsirius.so")" = '$ORIGIN'

if ldd "${PREFIX}/lib/libsirius.so" | grep 'not found'; then exit 1; fi

cmake -S tests/library-consumer -B tests/library-consumer/build -G Ninja \
  -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build tests/library-consumer/build
tests/library-consumer/build/sirius_consumer
