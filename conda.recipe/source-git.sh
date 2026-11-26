# Sourced by the build scripts with the DuckDB source directory to record. The
# source copy has no git history; give the checkout the commit it was copied
# from so the revision is recorded in the package.
duckdb_dir="$1"
if [ -n "$(git -C "${RECIPE_DIR}/../${duckdb_dir}" status --porcelain)" ]; then
  echo "${duckdb_dir} has local changes; its revision cannot be recorded" >&2
  exit 1
fi
duckdb_revision="$(git -C "${RECIPE_DIR}/../${duckdb_dir}" rev-parse HEAD)"
git -C "${SRC_DIR}/${duckdb_dir}" init -q
echo "${duckdb_revision}" > "$(git -C "${SRC_DIR}/${duckdb_dir}" rev-parse --absolute-git-dir)/HEAD"
