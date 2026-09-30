# Sirius DuckDB extension

This directory imports the setup from
[`sirius-db/sirius-duckdb`](https://github.com/sirius-db/sirius-duckdb/tree/ca1297c79bb4378f2a19464c5a7e61214e2812d9).
It builds the DuckDB extension against the installed shared `sirius` package.
DuckDB is temporarily pinned to the same 1.5.6 revision as the library; moving to
DuckDB v2 requires a separate compatibility change.

From the repository root, initialize the extension's dependencies:

```bash
git submodule update --init sirius-duckdb/duckdb sirius-duckdb/extension-ci-tools
```

Pixi builds and caches the [Sirius package](../packaging/conda/README.md) from
`../packaging/conda` as a source dependency using its `pixi-build`
preview feature. Initialize the library dependencies too, then build and test:

```bash
git submodule update --init duckdb substrait cucascade
cd sirius-duckdb
pixi run make release
SIRIUS_CONFIG_FILE="$PWD/test/config.yaml" pixi run make test
```

The lockfile includes the package's build and runtime dependencies. Tests require
an NVIDIA GPU and compatible driver. The initial environment targets Linux x86-64
with CUDA 13.4.

The output is `build/release/extension/sirius/sirius.duckdb_extension`. It requires
`libsirius.so` and its Conda runtime dependencies. Run the built DuckDB CLI with
`pixi run build/release/duckdb -unsigned` to use that environment.

CI builds on a CPU runner, copies Pixi's cached Sirius package, and transfers it
with the extension build to a GPU runner. The test job selects that binary package
in its local manifest so it cannot rebuild Sirius.

The Makefile uses the pinned `extension-ci-tools` Makefile helper. CI invokes
`make release` and `make test` directly rather than its hosted workflows.
The root build still provides the existing static distribution extension.
