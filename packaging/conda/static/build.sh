#!/usr/bin/env bash
set -euo pipefail
export CARGO_NET_OFFLINE=true
export CARGO_HOME="$SRC_DIR/.cargo-home"
export VCPKG_DISABLE_METRICS=1
: "${cuda_compiler_version:?conda-build must select a CUDA variant}"
export VCPKG_CUDA_VERSION="${cuda_compiler_version%%.*}"

# Restore only the pinned Git objects needed by vcpkg's builtin registry.
git clone --bare "$SRC_DIR/packaging/vendor/vcpkg.bundle" "$SRC_DIR/vcpkg/.git"
git -C "$SRC_DIR/vcpkg" config core.bare false
revision="$(cat "$SRC_DIR/vcpkg/.sirius-revision")"
test "$(git -C "$SRC_DIR/vcpkg" rev-parse HEAD)" = "$revision"
printf '%s\n' "$revision" > "$SRC_DIR/vcpkg/.git/shallow"
git -C "$SRC_DIR/vcpkg" remote set-url origin https://github.com/microsoft/vcpkg.git
"$SRC_DIR/vcpkg/bootstrap-vcpkg.sh" -disableMetrics
case "$(uname -m)" in
  x86_64) triplet=x64-linux ;;
  aarch64) triplet=arm64-linux ;;
  *) echo "Unsupported target architecture" >&2; exit 1 ;;
esac
cmake -S "$SRC_DIR" -B build-conda -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_CUDA_HOST_COMPILER="$CXX" \
  -DCMAKE_INSTALL_PREFIX="$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_TOOLCHAIN_FILE="$SRC_DIR/vcpkg/scripts/buildsystems/vcpkg.cmake" \
  -DVCPKG_TARGET_TRIPLET="$triplet" \
  -DVCPKG_INSTALLED_DIR="$SRC_DIR/vcpkg_installed" \
  -DVCPKG_INSTALL_OPTIONS=--clean-buildtrees-after-build \
  -DVCPKG_BUILD=ON -DCPM_LOCAL_PACKAGES_ONLY=ON \
  -DCMAKE_CUDA_ARCHITECTURES="75;80;86;89;90" \
  -DCMAKE_CXX_COMPILER_LAUNCHER= -DCMAKE_CUDA_COMPILER_LAUNCHER= \
  -DSIRIUS_BUILD_SHARED=OFF -DSIRIUS_BUILD_STATIC=ON \
  -DSIRIUS_BUILD_TESTS=OFF -DSIRIUS_BUILD_S3_TESTS=OFF
cmake --build build-conda --target sirius_static --parallel "$CPU_COUNT"
stage="$SRC_DIR/sirius-static-stage"
cmake --install build-conda --prefix "$stage" --component sirius_library
# Reject mismatched development metadata before producing a static package.
cmp "$stage/lib/cmake/sirius/sirius-duckdb-compatibility.cmake" \
    "$PREFIX/lib/cmake/sirius/sirius-duckdb-compatibility.cmake"
diff -r "$stage/include/sirius" "$PREFIX/include/sirius"
mkdir -p "$PREFIX/lib/cmake/sirius" "$PREFIX/share/sirius"
cp "$stage/lib/libsirius.a" "$PREFIX/lib/"
cp "$stage/lib/cmake/sirius/sirius-static-targets.cmake" \
   "$stage/lib/cmake/sirius/libsirius.a.cmake" "$PREFIX/lib/cmake/sirius/"
cp "$stage/share/sirius/libsirius.a.json" "$PREFIX/share/sirius/"
cp "$stage/share/sirius/input-manifest.json" "$PREFIX/share/sirius/static-input-manifest.json"
python "$SRC_DIR/packaging/collect-licenses.py" "$SRC_DIR" \
  "$PREFIX/share/licenses/sirius-static" --vcpkg-installed "$SRC_DIR/vcpkg_installed"
