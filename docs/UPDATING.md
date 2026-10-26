# Extension updating
When cloning this template, the target version of DuckDB should be the latest stable release of DuckDB. However, there
will inevitably come a time when a new DuckDB is released and the extension repository needs updating. This process goes
as follows:

- Bump submodules
  - `./duckdb` should be set to latest tagged release
  - Keep related submodules such as `./substrait` and `./sirius-duckdb/vcpkg` aligned when the release requires it
- Update the DuckDB package in `pixi.toml` to the matching release for Python users
- Bump versions in `.github/workflows`
  - `duckdb_version` input in `distribution.yml` should be set to latest tagged release
  - The reusable `sirius-db/extension-ci-tools` workflow ref and `ci_tools_version` input should be updated only when the remote CI tooling branch changes
- Set `DUCKDB_EXPLICIT_VERSION` to `vX.Y.Z` for the matching source commit in the
  engine provider, wrapper Makefile, and direct CI host builds. DuckDB otherwise
  reports a development version even when its source checkout is at a release tag.
