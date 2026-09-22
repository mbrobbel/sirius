#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Build local conda artifacts from a verified prepared source archive."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("--kind", choices=("shared", "static"), default="shared")
    parser.add_argument("--cuda", choices=("12.9", "13.3"), default="13.3")
    parser.add_argument("--output", type=Path, default=Path("build/conda"))
    parser.add_argument("--render-only", action="store_true")
    parser.add_argument(
        "--resolve",
        action="store_true",
        help="Resolve recipe dependencies while rendering",
    )
    args = parser.parse_args()
    source = args.source.resolve()
    manifest = json.loads(source.with_suffix(".json").read_text())
    with source.open("rb") as archive:
        digest = hashlib.sha256()
        for chunk in iter(lambda: archive.read(1024 * 1024), b""):
            digest.update(chunk)
        actual = digest.hexdigest()
    if actual != manifest["source_sha256"]:
        raise RuntimeError("Prepared source archive checksum mismatch")
    if args.kind == "static" and "vcpkg" not in manifest["revisions"]:
        raise RuntimeError("Prepare static package sources with --with-vcpkg")
    os.environ.update(
        SIRIUS_SOURCE_URL=source.as_uri(),
        SIRIUS_SOURCE_SHA256=actual,
        SIRIUS_SOURCE_REVISION=manifest["revision"],
    )
    os.environ.setdefault("CONDA_OVERRIDE_CUDA", args.cuda)
    root = Path(__file__).resolve().parent.parent
    recipe = root / "packaging/conda" / args.kind
    variants = {"cuda_compiler_version": args.cuda}
    variant_file = root / "packaging/conda/conda_build_config.yaml"
    if args.render_only:
        from conda_build import api
        from conda_build.config import Config

        config = Config(
            variant_config_files=[str(variant_file)],
            channel_urls=["rapidsai", "conda-forge"],
        )
        for metadata, _, _ in api.render(
            str(recipe),
            config=config,
            variants=variants,
            finalize=args.resolve,
            permit_unsatisfiable_variants=False,
            bypass_env_check=not args.resolve,
        ):
            print(api.output_yaml(metadata))
        return
    subprocess.run(
        [
            "conda-build",
            str(recipe),
            "--no-anaconda-upload",
            "--override-channels",
            "-c",
            "rapidsai",
            "-c",
            "conda-forge",
            "-m",
            str(variant_file),
            "--variants",
            json.dumps(variants),
            "--output-folder",
            str(args.output.resolve()),
        ],
        check=True,
    )


if __name__ == "__main__":
    main()
