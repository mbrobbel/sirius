#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Build and install Sirius before configuring its DuckDB consumer."""

import argparse
from pathlib import Path
import subprocess
import shutil


def run(*args):
    subprocess.run(args, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("preset")
    parser.add_argument("--targets", nargs="+", default=["sirius_shared"])
    parser.add_argument(
        "--extension-targets",
        nargs="+",
        default=["duckdb", "shell", "duckdb_local_extension_repo"],
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    build = root / "build" / args.preset
    cache = {}
    for line in (build / "CMakeCache.txt").read_text().splitlines():
        if line and not line.startswith(("#", "//")) and "=" in line:
            key, value = line.split("=", 1)
            cache[key.split(":", 1)[0]] = value
    bundled = cache.get("SIRIUS_BUILD_STATIC") == "ON"
    targets = args.targets + (["sirius_static"] if bundled else [])
    run("cmake", "--build", str(build), "--target", *targets)
    prefix = build / "install"
    run(
        "cmake",
        "--install",
        str(build),
        "--prefix",
        str(prefix),
        "--component",
        "sirius_library",
    )
    extension = build / "sirius-duckdb"
    package_dir = prefix / cache.get("CMAKE_INSTALL_LIBDIR", "lib") / "cmake/sirius"
    forwarded = [
        "CMAKE_BUILD_TYPE",
        "CMAKE_C_COMPILER",
        "CMAKE_CXX_COMPILER",
        "CMAKE_C_COMPILER_LAUNCHER",
        "CMAKE_CXX_COMPILER_LAUNCHER",
        "CMAKE_LINKER_TYPE",
        "CMAKE_C_FLAGS",
        "CMAKE_CXX_FLAGS",
        "CMAKE_EXE_LINKER_FLAGS",
        "CMAKE_SHARED_LINKER_FLAGS",
        "CMAKE_MODULE_LINKER_FLAGS",
        "ENABLE_SANITIZER",
        "ENABLE_UBSAN",
        "ENABLE_THREAD_SANITIZER",
    ]
    forwarded += [
        f"CMAKE_{lang}_FLAGS_{mode}"
        for lang in ("C", "CXX")
        for mode in ("DEBUG", "RELEASE", "RELWITHDEBINFO")
    ]
    run(
        "cmake",
        "-S",
        str(root / "duckdb"),
        "-B",
        str(extension),
        "-G",
        "Ninja",
        f"-Dsirius_DIR={package_dir}",
        f"-DDUCKDB_EXTENSION_CONFIGS={root / 'sirius-duckdb/extension_config.cmake'}",
        "-DOVERRIDE_GIT_DESCRIBE=v1.5.5",
        "-DEXTENSION_STATIC_BUILD=ON",
        f"-DSIRIUS_DUCKDB_LINKAGE={'static' if bundled else 'shared'}",
        "-DCMAKE_CXX_SCAN_FOR_MODULES=OFF",
        *(f"-D{key}={cache[key]}" for key in forwarded if key in cache),
    )
    run("cmake", "--build", str(extension), "--target", *args.extension_targets)
    if bundled:
        # extension-ci-tools collects distribution artifacts at these paths.
        artifact = extension / "extension/sirius/sirius.duckdb_extension"
        if artifact.is_file():
            destination = build / "extension/sirius"
            destination.mkdir(parents=True, exist_ok=True)
            shutil.copy2(artifact, destination / artifact.name)
        repository = extension / "repository"
        if repository.is_dir():
            shutil.copytree(repository, build / "repository", dirs_exist_ok=True)


if __name__ == "__main__":
    main()
