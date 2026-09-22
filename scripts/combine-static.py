#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Combine redistributable archives; retain only platform and driver dependencies."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

PLATFORM_LIBRARIES = {
    "c",
    "m",
    "dl",
    "rt",
    "pthread",
    "util",
    "resolv",
    "atomic",
    "gcc_s",
    "stdc++",
    "stdc++fs",
    "cuda",
    "nvidia-ml",
}


def library_name(path):
    name = path.name.removeprefix("lib")
    return name.split(".so", 1)[0].removesuffix(".a")


def collect(graph, roots):
    records = {}
    for manifest in graph.glob("*.txt"):
        record = dict(line.split("=", 1) for line in manifest.read_text().splitlines())
        records[record["name"]] = record
    seen, artifacts, platform = set(), [], []

    def visit(item):
        if not item or item in seen or item.startswith("::@"):
            return
        seen.add(item)
        if item in records:
            record = records[item]
            for artifact in record["file"].split(";"):
                visit(artifact)
            for dependency in record["links"].split(";"):
                visit(dependency)
            return
        name = item.removeprefix("-l")
        path = Path(item)
        if path.is_absolute():
            name = library_name(path)
        if name in PLATFORM_LIBRARIES:
            if name not in platform:
                platform.append(name)
        elif path.is_absolute() and path.suffix in (".a", ".o") and path.is_file():
            artifacts.append(path)
        elif item in ("-pthread", "-Wl,--as-needed", "-Wl,--no-as-needed"):
            if item == "-pthread" and "pthread" not in platform:
                platform.append("pthread")
        else:
            raise ValueError(f"Non-static or unsupported dependency in bundle: {item}")

    for root in roots:
        visit(root)
    return artifacts, platform


def combine(ar, output, artifacts):
    if not artifacts:
        raise ValueError("Cannot create an empty Sirius archive")
    output.parent.mkdir(parents=True, exist_ok=True)
    # Safe local names avoid MRI quoting and retain duplicate archive members.
    with tempfile.TemporaryDirectory(
        prefix="sirius-ar-", dir=output.parent
    ) as directory:
        directory = Path(directory)
        lines = ["CREATE combined.a"]
        for index, path in enumerate(reversed(artifacts)):
            local = directory / f"input-{index}{path.suffix}"
            local.symlink_to(path.resolve())
            command = "ADDLIB" if path.suffix == ".a" else "ADDMOD"
            lines.append(f"{command} {local.name}")
        lines.extend(["SAVE", "END", ""])
        subprocess.run(
            [ar, "-M"], input="\n".join(lines), text=True, cwd=directory, check=True
        )
        temporary = directory / "combined.a"
        subprocess.run([ar, "sD", str(temporary)], check=True)
        temporary.replace(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ar", required=True)
    parser.add_argument("--graph", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--roots", nargs="+", required=True)
    args = parser.parse_args()
    artifacts, platform = collect(args.graph, args.roots)
    combine(args.ar, args.output, artifacts)
    args.output.with_suffix(".a.cmake").write_text(
        'set(SIRIUS_STATIC_SYSTEM_LIBRARIES "' + ";".join(platform) + '")\n'
    )
    inventory = []
    for path in artifacts:
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        inventory.append({"name": path.name, "sha256": digest})
    args.output.with_suffix(".a.json").write_text(
        json.dumps({"archives": inventory, "system_libraries": platform}, indent=2)
        + "\n"
    )


if __name__ == "__main__":
    main()
