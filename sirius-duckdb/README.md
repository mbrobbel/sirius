# Sirius DuckDB extension

This directory imports the setup from
[`sirius-db/sirius-duckdb`](https://github.com/sirius-db/sirius-duckdb/tree/ca1297c79bb4378f2a19464c5a7e61214e2812d9).
It builds the DuckDB extension against the installed `sirius-static` package.
DuckDB is temporarily pinned to the same 1.5.6 revision as the library; moving to
DuckDB v2 requires a separate compatibility change.

From the repository root, initialize the extension's dependencies:

```bash
git submodule update --init sirius-duckdb/duckdb sirius-duckdb/extension-ci-tools
```

Pixi builds and caches the static [Sirius package](../conda.recipe/README.md) from
`../conda.recipe` as a source dependency using its `pixi-build`
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

The output is `build/release/extension/sirius/sirius.duckdb_extension`. It bundles
Sirius and its dependencies; it requires only NVIDIA driver and base OS libraries
at runtime. Run the built DuckDB CLI with
`pixi run build/release/duckdb -unsigned`.

CI consumes the CUDA 13 `sirius-static` and `sirius-devel` packages from the
package matrix on a CPU runner, then transfers the extension build to a GPU runner.
Neither job recompiles Sirius. The test environment contains the tools and C++
runtime needed by DuckDB's test executable.

The Makefile uses the pinned `extension-ci-tools` Makefile helper. CI invokes
`make release` and `make test` directly rather than its hosted workflows.
The root build still provides the existing static distribution extension.
