#!/usr/bin/env bash
set -euo pipefail

extension="${PREFIX}/lib/sirius.duckdb_extension"
test -f "${extension}"
test -f "${PREFIX}/etc/conda/activate.d/sirius-duckdb.sh"
test -f "${PREFIX}/etc/conda/deactivate.d/sirius-duckdb.sh"

if readelf -d "${extension}" | grep -E 'lib(cudf|rmm|cuvs|nvrtc|nvcomp|sirius|cublas|cusolver|cusparse|curand|nvJitLink|gomp)[.]so'; then
  echo "the bundled extension retains a user-space shared dependency" >&2
  exit 1
fi
test -z "$(patchelf --print-rpath "${extension}")"

SIRIUS_DISABLE=1 duckdb -unsigned -c \
  "LOAD '${extension}'; SELECT loaded FROM duckdb_extensions() WHERE extension_name = 'sirius';"
