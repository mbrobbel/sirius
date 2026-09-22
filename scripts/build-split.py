#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Build and install Sirius before configuring its DuckDB consumer."""

import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("preset")
    parser.add_argument("--cmake", default="cmake")
    parser.add_argument("--duckdb-dir", type=Path, default=Path("duckdb"))
    parser.add_argument("--targets", nargs="+", default=["sirius_shared"])
    parser.add_argument(
        "--extension-targets",
        nargs="+",
        default=["duckdb", "duckdb_local_extension_repo"],
    )
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent

    def run(*command):
        subprocess.run((args.cmake, *command), check=True)

    build = root / "build" / args.preset
    cache = {}
    for line in (build / "CMakeCache.txt").read_text().splitlines():
        if line and not line.startswith(("#", "//")) and "=" in line:
            key, value = line.split("=", 1)
            cache[key.split(":", 1)[0]] = value
    run("--build", str(build), "--target", *args.targets)
    prefix = build / "install"
    run(
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
        "CMAKE_EXPORT_COMPILE_COMMANDS",
        "CMAKE_TOOLCHAIN_FILE",
        "CMAKE_PREFIX_PATH",
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
        "EXPORT_DYNAMIC_SYMBOLS",
        "OVERRIDE_GIT_DESCRIBE",
        "VCPKG_BUILD",
        "VCPKG_MANIFEST_DIR",
        "VCPKG_INSTALLED_DIR",
        "VCPKG_TARGET_TRIPLET",
        "VCPKG_HOST_TRIPLET",
    ]
    forwarded += [
        f"CMAKE_{lang}_FLAGS_{mode}"
        for lang in ("C", "CXX")
        for mode in ("DEBUG", "RELEASE", "RELWITHDEBINFO")
    ]
    run(
        "-S",
        str(root / args.duckdb_dir),
        "-B",
        str(extension),
        "-G",
        "Ninja",
        f"-Dsirius_DIR={package_dir}",
        f"-DDUCKDB_EXTENSION_CONFIGS={root / 'sirius-duckdb/extension_config.cmake'}",
        "-DOVERRIDE_GIT_DESCRIBE=v1.5.6",
        "-DEXTENSION_STATIC_BUILD=ON",
        "-DCMAKE_CXX_SCAN_FOR_MODULES=OFF",
        *(f"-D{key}={cache[key]}" for key in forwarded if key in cache),
    )
    run("--build", str(extension), "--target", *args.extension_targets)


if __name__ == "__main__":
    main()
