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
