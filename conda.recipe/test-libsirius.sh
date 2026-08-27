#!/usr/bin/env bash
set -euo pipefail

test -f "${PREFIX}/include/sirius/sirius_ffi.hpp"
test -f "${PREFIX}/lib/cmake/Sirius/SiriusConfig.cmake"
test -f "${PREFIX}/lib/libsirius.so"
test "$(patchelf --print-rpath "${PREFIX}/lib/libsirius.so")" = '$ORIGIN'

cmake -S library-consumer -B library-consumer/build -G Ninja \
  -DCMAKE_PREFIX_PATH="${PREFIX}"
cmake --build library-consumer/build
library-consumer/build/libsirius_consumer
