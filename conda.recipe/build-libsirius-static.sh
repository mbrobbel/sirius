#!/usr/bin/env bash
set -euo pipefail

source "${RECIPE_DIR}/cuda-archs.sh"

export LIBCLANG_PATH="${BUILD_PREFIX}/lib"
export CLANG_PATH="${BUILD_PREFIX}/bin/clang"

case "$(uname -m)" in
  x86_64) triplet=x64-linux-release ;;
  aarch64) triplet=arm64-linux-release ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

wrapper="${SRC_DIR}/sirius-duckdb"

# The overlay port enumerates its source inputs with git; the source copy has no history.
for repository in . cucascade substrait; do
  git -C "${SRC_DIR}/${repository}" init -q
done
source "${RECIPE_DIR}/source-git.sh" sirius-duckdb/duckdb
export DUCKDB_COMMIT="${duckdb_revision}"

# The source copy has no git history, which vcpkg's builtin baseline needs.
vcpkg_commit="$(sed -n 's/.*"builtin-baseline": *"\([0-9a-f]*\)".*/\1/p' "${wrapper}/vcpkg.json")"
vcpkg="${SRC_DIR}/../vcpkg-checkout"
rm -rf "${vcpkg}"
git clone --filter=blob:none https://github.com/microsoft/vcpkg.git "${vcpkg}"
git -C "${vcpkg}" checkout "${vcpkg_commit}"

# Conda's CUDA activation puts the toolkit's older CCCL ahead of the vcpkg one.
for flags in CFLAGS CXXFLAGS CPPFLAGS; do
  export "${flags}=$(printf '%s' "${!flags}" | sed -E 's# ?-I[^ ]*/targets/[^ ]*/include##g')"
done

export VCPKG_CUDA_VERSION="${cuda_version}"
export VCPKG_CUDA_ARCHITECTURES="${cuda_archs}"
export VCPKG_DISABLE_METRICS=1
"${vcpkg}/bootstrap-vcpkg.sh" -disableMetrics

# The overlay `sirius` port builds this checkout with SIRIUS_BUILD_STATIC=ON.
"${vcpkg}/vcpkg" install \
  --x-manifest-root="${wrapper}" \
  --x-install-root="${SRC_DIR}/vcpkg_installed" \
  --overlay-ports="${wrapper}/vcpkg_ports" \
  --overlay-triplets="${wrapper}/vcpkg_triplets" \
  --triplet="${triplet}" \
  --host-triplet="${triplet}" \
  --clean-buildtrees-after-build

dest="${PREFIX}/lib/sirius-static"
installed="${SRC_DIR}/vcpkg_installed/${triplet}"
"${RECIPE_DIR}/package-static.sh" "${installed}" "${dest}" "${duckdb_revision}" "${cuda_version}"

mkdir -p "${PREFIX}/share/licenses/libsirius-static"
for copyright in "${installed}"/share/*/copyright; do
  name="$(basename "$(dirname "${copyright}")")"
  cp "${copyright}" "${PREFIX}/share/licenses/libsirius-static/${name}.txt"
done
