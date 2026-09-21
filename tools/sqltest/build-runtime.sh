#!/usr/bin/env bash
set -euo pipefail

revision=${1:?DuckDB revision required}
version=${2:?DuckDB version required}
destination=${3:?output directory required}
[[ "$revision" =~ ^[0-9a-f]{40}$ && "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]
mkdir -p "$destination"
destination=$(realpath "$destination")
recipe=$(sha256sum "${BASH_SOURCE[0]}" | cut -d ' ' -f 1)
identity="$revision $version $recipe"
if [[ -f "$destination/identity" && $(cat "$destination/identity") == "$identity" && -f "$destination/lib/libduckdb.so" ]]; then
  exit 0
fi

build_root=$(mktemp -d)
trap 'rm -rf "$build_root"' EXIT
mkdir "$build_root/source"
gh api "repos/sirius-db/duckdb/tarball/$revision" > "$build_root/source.tar.gz"
tar -xzf "$build_root/source.tar.gz" --strip-components=1 -C "$build_root/source"
cmake -S "$build_root/source" -B "$build_root/build" -G Ninja \
  -DCMAKE_BUILD_TYPE=Release -DBUILD_UNITTESTS=OFF -DBUILD_SHELL=OFF \
  -DDUCKDB_EXTENSION_CONFIGS= -DBUILD_EXTENSIONS='icu;tpch;tpcds' \
  -DOVERRIDE_GIT_DESCRIBE="$version" -DGIT_COMMIT_HASH="${revision:0:10}" \
  -DDUCKDB_EXPLICIT_PLATFORM=linux_amd64
cmake --build "$build_root/build" --target duckdb --parallel "${SQLTEST_BUILD_JOBS:-4}"
mkdir -p "$destination/lib" "$destination/include"
cp "$build_root/build/src/libduckdb.so" "$destination/lib/"
cp "$build_root/source/src/include/duckdb.h" "$destination/include/"
printf '%s\n' "$identity" > "$destination/identity"
