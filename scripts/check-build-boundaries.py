#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Check object reuse, wrapper isolation, and a relocated installed consumer."""

import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import tempfile


def run(*args):
    subprocess.run(args, check=True)


def check_graph(engine_build, wrapper_build, root):
    engine = json.loads((engine_build / "compile_commands.json").read_text())
    # The GPU-free NVTX fixture intentionally recompiles just its helper.
    production = [
        entry for entry in engine if "injection_objects.dir/" not in entry["command"]
    ]
    counts = Counter(entry["file"] for entry in production)
    common = [entry for entry in engine if "sirius_objects.dir/" in entry["command"]]
    if not common or any(counts[entry["file"]] != 1 for entry in common):
        raise RuntimeError("Engine implementation must compile exactly once")
    wrapper = json.loads((wrapper_build / "compile_commands.json").read_text())
    for entry in wrapper:
        source = Path(entry["file"])
        if any(
            source.is_relative_to(root / part)
            for part in ("src", "rust", "cucascade", "substrait")
        ):
            raise RuntimeError(f"Wrapper compiles engine dependency: {source}")
    print(f"All {len(common)} engine compilation units are shared; wrapper is isolated")


def check_install(build, root):
    with tempfile.TemporaryDirectory(prefix="sirius-install-") as temporary:
        temporary = Path(temporary)
        original = temporary / "original"
        relocated = temporary / "relocated"
        run(
            "cmake",
            "--install",
            str(build),
            "--prefix",
            str(original),
            "--component",
            "sirius_library",
        )
        original.rename(relocated)
        for metadata in relocated.rglob("*.cmake"):
            contents = metadata.read_text()
            if str(root) in contents or str(original) in contents:
                raise RuntimeError(f"Non-relocatable package metadata: {metadata}")
        consumer = temporary / "consumer"
        run(
            "cmake",
            "-S",
            str(root / "test/cmake/installed_consumer"),
            "-B",
            str(consumer),
            "-G",
            "Ninja",
            f"-DCMAKE_PREFIX_PATH={relocated}",
        )
        run("cmake", "--build", str(consumer))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("build", type=Path)
    parser.add_argument("--wrapper-build", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    build = args.build.resolve()
    check_graph(build, args.wrapper_build or build / "sirius-duckdb", root)
    check_install(build, root)


if __name__ == "__main__":
    main()
