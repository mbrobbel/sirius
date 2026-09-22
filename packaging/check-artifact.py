#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Reject accidental shared GPU dependencies in the bundled extension."""

import argparse
from pathlib import Path
import re
import subprocess


def check(path, linkage):
    dynamic = subprocess.check_output(["readelf", "-d", str(path)], text=True)
    needed = re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic)
    if linkage == "shared":
        if not any(name.startswith("libsirius.so") for name in needed):
            raise RuntimeError("Shared wrapper does not depend on Sirius")
    else:
        platform = re.compile(
            r"lib(c|m|dl|rt|pthread|util|resolv|atomic|gcc_s|stdc\+\+|cuda|nvidia-ml)\.so(?:\..*)?"
            r"|ld-linux-.*\.so(?:\..*)?"
        )
        unexpected = [name for name in needed if not platform.fullmatch(name)]
        if unexpected:
            raise RuntimeError(
                f"Bundled extension has shared dependencies: {unexpected}"
            )
    symbols = subprocess.check_output(
        ["nm", "-D", "--defined-only", str(path)], text=True
    )
    if "sirius_duckdb_cpp_init" not in symbols:
        raise RuntimeError("Missing DuckDB extension entry point")
    print(f"{linkage} extension dependencies: {', '.join(needed)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("linkage", choices=("shared", "static"))
    args = parser.parse_args()
    check(args.artifact, args.linkage)


if __name__ == "__main__":
    main()
