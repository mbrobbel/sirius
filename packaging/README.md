# Sirius package inputs

Packaging uses committed source revisions. Uncommitted changes are intentionally
excluded. Prepare the source once, then pass its path and SHA-256 to the recipe;
package builds do not clone the repository or resolve moving Git references.

```bash
git submodule update --init --recursive duckdb cucascade substrait vcpkg
pixi run python scripts/prepare-package-source.py build/sirius-source.tar.gz --with-vcpkg
```

The archive contains the pinned submodules, cuCollections, Corrosion, and Cargo's
vendored workspace dependencies. `.sirius-revision` files allow source-archive
builds to record and check the DuckDB revision without Git metadata. The packaged
`share/sirius/input-manifest.json` records revisions and input lock-file hashes.
The sidecar JSON also records the final archive's SHA-256.

The preparation command needs network access to fetch verified inputs and fill
Cargo's cache. The CMake and Cargo portions of the package build use the prepared
sources without network access. The static recipe additionally uses the pinned
vcpkg baseline and checked-in overlays; use a vcpkg binary/download cache for
repeat builds. It must not replace the bundled archive with conda shared GPU
libraries.

Archive member order, ownership, and timestamps are normalized to the selected
commit's timestamp. Keep the archive and sidecar with CI artifacts. These steps
prepare and test artifacts; they do not publish a conda channel.

## Shared conda packages

The shared recipe emits `libsirius` (runtime library and its target export) and
`libsirius-devel` (public headers and common CMake package metadata). Install the
development package with a library package. Keeping common metadata independent
of the runtime allows a static-only consumer environment.

```bash
pixi exec --spec conda-build=26.7.1 -- python scripts/build-conda.py \
  build/sirius-source.tar.gz --kind shared --cuda 13.3 --render-only
pixi exec --spec conda-build=26.7.1 -- python scripts/build-conda.py \
  build/sirius-source.tar.gz --kind shared --cuda 13.3
```

Use `--cuda 12.9` for CUDA 12. Native Linux x86-64 and aarch64 builds use the same
recipe. Runtime dependency bounds come from the host packages' run exports; the
NVIDIA driver is represented by `__cuda`. Tests compile an installed consumer
without CUDA headers and do not initialize a GPU. The build driver always disables
channel upload and verifies the source archive checksum before invoking conda-build.

The recipes use conda-build's [multiple outputs](https://docs.conda.io/projects/conda-build/en/stable/resources/define-metadata.html#outputs-section)
so each package has an explicit file list.

## Static conda package

After building the shared recipe into the local output channel, build the static
recipe with the same source archive and CUDA variant:

```bash
pixi exec --spec conda-build=26.7.1 -- python scripts/build-conda.py \
  build/sirius-source.tar.gz --kind static --cuda 13.3
```

`libsirius-static` contains the combined archive, static CMake target, external
link metadata, dependency hashes, and third-party notices. It depends on the
matching `libsirius-devel` revision, whose headers and ABI metadata are checked
against the static build. It does not depend on shared Sirius or shared RAPIDS
packages. Its test environment links an installed consumer and checks that shared
Sirius and cuDF are absent. NVIDIA driver stubs are needed at link time; deployed
applications use the host driver.

Static source preparation also records a Git bundle for vcpkg's pinned registry.
The recipe restores that bundle locally; subsequent port assets and historical
registry objects are fetched by vcpkg using the pinned baseline and overlays.
`VCPKG_BINARY_SOURCES` and `VCPKG_DOWNLOADS` can point to existing build caches.
The recipe cannot substitute conda shared GPU libraries for missing static
dependencies: archive assembly rejects them.

## CI and consumer checks

The `Sirius packages` workflow prepares one source artifact and builds native
Linux x86-64/aarch64 packages for CUDA 12.9 and 13.3. It runs on relevant changes
to `dev` and can be dispatched manually. It uploads artifacts without publishing
a channel. Each build links C++ and Rust consumers against installed packages,
then builds the independent DuckDB wrapper in both linkage modes. ELF inspection
rejects shared GPU dependencies in the bundled extension. Separate x86-64 GPU
jobs load both extensions and check transparent and explicit GPU query results.
GPU execution on aarch64 requires an additional GPU runner.

To exercise the same installed consumers against a local output channel:

```bash
pixi exec --spec conda-build=26.7.1 -- bash packaging/test-consumers.sh \
  build build/conda shared 13.3
pixi exec --spec conda-build=26.7.1 -- bash packaging/test-consumers.sh \
  gpu build/conda shared 13.3
```

Use `static` for the combined archive. The `gpu` phase requires the wrapper
artifacts from the `build` phase and a working NVIDIA driver. The ordinary engine
CI retains the full C++ integration suite; package smoke tests complement it.
