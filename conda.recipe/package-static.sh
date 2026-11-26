#!/usr/bin/env bash
# Usage: package-static.sh <vcpkg prefix> <destination> <duckdb revision> <cuda major>
#
# Merges every static archive of the vcpkg prefix into one libsirius.a. The CUDA
# toolkit libraries are left out; consumers link them from their own toolkit.
set -euo pipefail

prefix="$1"
dest="$2"
duckdb_revision="$3"
cuda_version="$4"
recipe_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "${dest}/lib" "${dest}/share/sirius"

script="$(mktemp)"
echo "CREATE ${dest}/lib/libsirius.a" > "${script}"
for archive in "${prefix}"/lib/*.a; do
  case "$(basename "${archive}")" in
    libsirius.a) ;;
    libcublas_static.a | libcublasLt_static.a | libcusolver_static.a | \
      libcusolver_lapack_static.a | libcusolver_metis_static.a | \
      libcusparse_static.a | libcurand_static.a | libculibos.a | \
      libnvJitLink_static.a | libnvrtc_static.a | libnvrtc-builtins_static.a) continue ;;
  esac
  echo "ADDLIB ${archive}" >> "${script}"
done
printf 'SAVE\nEND\n' >> "${script}"
"${AR:-ar}" -M < "${script}"
"${RANLIB:-ranlib}" "${dest}/lib/libsirius.a"
rm "${script}"

# Conda's CUDA packages do not ship culibos, which the cuBLAS and cuSOLVER static libraries need.
cp "${prefix}/lib/libculibos.a" "${dest}/lib/"

# UCX transports register themselves from constructors, so the consumer's link
# must keep them whole instead of pulling members out of the merged archive.
mkdir -p "${dest}/lib/ucx"
cp "${prefix}"/lib/ucx/*.a "${dest}/lib/ucx/"

mkdir -p "${dest}/include"
cp -a "${prefix}/include/sirius" "${prefix}/include/sirius-c" "${dest}/include/"

cp "${prefix}/share/sirius/sirius-cuda-fatbin.ld" \
  "${prefix}/share/sirius/sirius-duckdb-revision.cmake" \
  "${prefix}/share/sirius/sirius-config-version.cmake" \
  "${dest}/share/sirius/"
sed -e "s/@DUCKDB_REVISION@/${duckdb_revision}/" -e "s/@CUDA_VERSION@/${cuda_version}/" \
  "${recipe_dir}/sirius-static-config.cmake.in" \
  > "${dest}/share/sirius/sirius-config.cmake"
