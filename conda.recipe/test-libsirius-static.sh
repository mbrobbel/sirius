#!/usr/bin/env bash
set -euo pipefail

archive="${PREFIX}/lib/sirius-static/libsirius.a"
dependencies="${PREFIX}/lib/sirius-static/libsirius_dependencies.a"
test -s "${archive}"
test -s "${dependencies}"
test -f "${PREFIX}/lib/cmake/SiriusStatic/SiriusStaticConfig.cmake"
members="$(ar t "${archive}")"
grep -q 'sirius_ffi.cpp.o' <<<"${members}"
if grep -q 'sirius_duckdb_entry.cpp.o' <<<"${members}"; then
  echo "the static bundle contains the DuckDB entry point" >&2
  exit 1
fi
nm -C "${archive}" | grep 'sirius::ffi::make_context()' >/dev/null
