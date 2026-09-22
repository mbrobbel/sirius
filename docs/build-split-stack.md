# Sirius build split

The implementation for [#1734](https://github.com/sirius-db/sirius/issues/1734)
and [#1733](https://github.com/sirius-db/sirius/issues/1733) is divided into the
following dependent branches. Each branch includes the previous one; review its
difference against that predecessor. The first branch starts at `dev` commit
`f4fb78ba8`. Branches are on `mbrobbel/sirius`.

| Order | Branch (`build/sirius/` prefix) | Change |
| --- | --- | --- |
| 1 | `01-build-definitions` | Extract dependency, source, target, install and test definitions |
| 2 | `02-registration-boundary` | Public registration boundary and thin extension entry point |
| 3 | `03-shared-objects` | One engine object target; runtime NVTX output detection |
| 3.1 | `03.1-directory-sources` | Declare engine sources in subsystem directories |
| 4 | `04-duckdb-provider` | Isolate the temporary DuckDB source dependency |
| 5 | `05-installed-package` | Export relocatable CMake targets and ABI checks |
| 6 | `06-standalone-build` | Root Sirius CMake project and separate DuckDB consumer |
| 7 | `07-test-ownership` | Engine C++ tests and wrapper SQL tests in their own builds |
| 8 | `08-consumer-ci` | Check object reuse, wrapper isolation and relocated consumers |
| 9 | `09-static-bundle` | Combine the transitive static archive closure |
| 10 | `10-bundled-extension` | Build a bundled DuckDB extension from the installed archive |
| 11 | `11-installed-consumers` | Migrate Rust, scripts and documentation to installed Sirius |
| 12 | `12-package-inputs` | Prepare pinned source inputs and vendored Cargo dependencies |
| 13 | `13-shared-conda` | Shared runtime and common development conda packages |
| 14 | `14-static-conda` | Static conda package using the same development metadata |
| 15 | `15-package-ci` | Native package matrix and installed consumer/GPU checks |

## Build boundaries

The root CMake project owns `sirius_objects`, the shared library, the combined
static archive, installation and engine tests. Production C++/CUDA implementation
sources compile once. Each engine subsystem declares its sources locally with
`target_sources(sirius_objects PRIVATE ...)`; common target settings stay
centralized. NVTX uses the loaded object's runtime identity to distinguish an
executable from a shared library, including PIE executables.

`sirius-duckdb/` contains the DuckDB extension entry point, headers, build
configuration and SQL tests. It uses DuckDB extension helpers and imports an
installed `sirius` package; it does not compile engine sources or GPU dependencies.
It can link either the shared library or the combined archive. Revision,
compiler, build mode and libstdc++ ABI checks reject incompatible installations.

The root build still compiles a pinned DuckDB source dependency because Sirius
uses internal DuckDB APIs. That dependency is isolated behind
`sirius::duckdb_dependency`; replacing it with an upstream installed DuckDB
package is a subsequent adapter change. DuckDB extension helpers do not create
the Sirius library targets.

## Validation

Local validation covers the native shared library, engine unit-test executable
build, shared wrapper, relocated installed C++ consumer and Rust test executable
linking. Build-graph checks verify that all 313 engine compilation units are
shared and that the wrapper compiles no engine dependencies. CPU tests cover
runtime NVTX in PIE, non-PIE and shared-library outputs, static archive closure,
registration retention, host registration precedence and installed static targets.
A negative archive test rejects shared dependencies. Prepared-source tests check
determinism and missing submodules. Shared conda recipes resolve for CUDA 12.9
and 13.3; static recipes render.

A full vcpkg static build, conda package builds, aarch64 builds and GPU execution
require the package workflow and appropriate runners. They have not been
validated locally. The workflow uploads local artifacts and never publishes a
conda channel. See [packaging instructions](../packaging/README.md) for the native
matrix, installed C++/Rust consumers, ELF inspection and GPU loading checks.
