#!/usr/bin/env bash
set -euo pipefail

case "${cuda_version}" in
  12) cuda_archs="75-real;80-real;86-real;89-real;90a-real" ;;
  13) cuda_archs="75-real;80-real;86-real;89-real;90a-real;100f-real;120a-real;120" ;;
  *) echo "unsupported CUDA version: ${cuda_version}" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  x86_64) triplet=x64-linux-release ;;
  aarch64) triplet=arm64-linux-release ;;
  *) echo "unsupported native architecture: $(uname -m)" >&2; exit 1 ;;
esac

test -f "${SRC_DIR}/vcpkg/bootstrap-vcpkg.sh"
test -f "${SRC_DIR}/duckdb/CMakeLists.txt"

export VCPKG_CUDA_VERSION="${cuda_version}"
export VCPKG_DISABLE_METRICS=1
"${SRC_DIR}/vcpkg/bootstrap-vcpkg.sh" -disableMetrics

cmake -S "${SRC_DIR}/duckdb" -B build-libsirius-static -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_CUDA_ARCHITECTURES="${cuda_archs}" \
  -DCMAKE_INSTALL_PREFIX="${PREFIX}" \
  -DCMAKE_SKIP_RPATH=ON \
  -DCMAKE_TOOLCHAIN_FILE="${SRC_DIR}/vcpkg/scripts/buildsystems/vcpkg.cmake" \
  -DCPM_LOCAL_PACKAGES_ONLY=ON \
  -DDUCKDB_EXTENSION_CONFIGS="${SRC_DIR}/extension_config.cmake" \
  -DEXPORT_DYNAMIC_SYMBOLS=ON \
  -DEXTENSION_STATIC_BUILD=ON \
  -DOVERRIDE_GIT_DESCRIBE=v1.5.5 \
  -DSIRIUS_BUILD_S3_TESTS=OFF \
  -DSIRIUS_BUILD_SHARED_LIBRARY=OFF \
  -DSIRIUS_EXTENSION_VERSION="${PKG_VERSION}" \
  -DSIRIUS_PACKAGE_VERSION="${PKG_VERSION}" \
  -DVCPKG_BUILD=ON \
  -DVCPKG_INSTALL_OPTIONS=--clean-buildtrees-after-build \
  -DVCPKG_MANIFEST_DIR="${SRC_DIR}" \
  -DVCPKG_TARGET_TRIPLET="${triplet}"

cmake --build build-libsirius-static --target sirius_static_bundle -j"${CPU_COUNT}"
cmake --install build-libsirius-static --component SiriusStatic

license_dir="${PREFIX}/share/licenses/libsirius-static/vcpkg"
mkdir -p "${license_dir}"
while IFS= read -r copyright_file; do
  package_name="$(basename "$(dirname "${copyright_file}")")"
  cp "${copyright_file}" "${license_dir}/${package_name}.txt"
done < <(find "build-libsirius-static/vcpkg_installed/${triplet}/share" -name copyright -type f | sort)
