#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${CONDA_PREFIX:-}" ]]; then
  exit 0
fi

clang_cpp="$CONDA_PREFIX/bin/clang-cpp"
clang_pp="$CONDA_PREFIX/bin/clang++"

if [[ -x "$clang_cpp" ]]; then
  ln -sf "$clang_cpp" "$clang_pp"
fi

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rm -f "$project_root/duckdb/CMakePresets.json"
printf '%s\n' \
  '{' \
  '  "version": 6,' \
  '  "include": ["../cmake/CMakePresets.json"]' \
  '}' \
  > "$project_root/duckdb/CMakeUserPresets.json"

mkdir -p build
pixi shell-hook -s bash > $project_root/build/sirius_pixi_env_for_clion.sh
