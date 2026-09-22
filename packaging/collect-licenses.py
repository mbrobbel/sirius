#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Collect notices for embedded source dependencies and installed vcpkg ports."""

import argparse
from pathlib import Path
import shutil


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--vcpkg-installed", type=Path)
    args = parser.parse_args()
    roots = [
        args.source / name
        for name in (
            "duckdb",
            "cucascade",
            "substrait",
            "rust/vendor",
            "packaging/vendor",
        )
    ]
    args.output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(args.source / "LICENSE", args.output / "LICENSE")
    for root in roots:
        for path in root.rglob("*"):
            if path.is_file() and path.name.upper().startswith(
                ("LICENSE", "COPYING", "COPYRIGHT", "NOTICE")
            ):
                destination = args.output / path.relative_to(args.source)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, destination)
    if args.vcpkg_installed:
        for path in args.vcpkg_installed.glob("*/share/*/copyright"):
            destination = args.output / "vcpkg" / path.relative_to(args.vcpkg_installed)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, destination)


if __name__ == "__main__":
    main()
