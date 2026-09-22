#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
"""Create a deterministic source archive with pinned submodules and vendored Cargo inputs."""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

CORROSION_REVISION = "1499b14e4906a2890f5cee1547c8848db261753d"
CUCO_REVISION = "0883368d39296f3bef3a058033141bcc642c5c54"
CUCO_SHA256 = "4ec8320a0372839b991f0b431c7f8bf0e770006cb3c8631c6e373c434471fd45"


def git(repository, *arguments):
    return (
        subprocess.check_output(["git", "-C", str(repository), *arguments])
        .decode()
        .strip()
    )


def submodules(repository, revision):
    result = {}
    for line in git(repository, "ls-tree", "-r", revision).splitlines():
        metadata, path = line.split("\t", 1)
        if metadata.startswith("160000 commit "):
            result[path] = metadata.split()[2]
    return result


def snapshot(repository, revision, destination, revisions, prefix="", recurse=True):
    if (
        Path(git(repository, "rev-parse", "--show-toplevel")).resolve()
        != repository.resolve()
    ):
        raise RuntimeError(f"Initialize the submodule at {repository} before packaging")
    destination.mkdir(parents=True, exist_ok=True)
    archive = subprocess.check_output(
        ["git", "-C", str(repository), "archive", revision]
    )
    with tarfile.open(fileobj=io.BytesIO(archive)) as source:
        members = [
            member
            for member in source.getmembers()
            if member.name.split("/", 1)[0] not in (".agents", ".claude", ".codex")
        ]
        source.extractall(destination, members=members, filter="data")
    revisions[prefix or "."] = revision
    (destination / ".sirius-revision").write_text(revision + "\n")
    if recurse:
        for path, pinned in submodules(repository, revision).items():
            snapshot(
                repository / path,
                pinned,
                destination / path,
                revisions,
                str(Path(prefix) / path),
            )


def checksum(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def archive_tree(source, output, epoch):
    with output.open("wb") as raw, gzip.GzipFile(
        fileobj=raw, mode="wb", filename="", mtime=epoch
    ) as compressed:
        with tarfile.open(fileobj=compressed, mode="w") as archive:
            for path in sorted(source.rglob("*")):
                info = archive.gettarinfo(
                    str(path), arcname=f"sirius/{path.relative_to(source)}"
                )
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                info.pax_headers = {}
                if info.isfile():
                    with path.open("rb") as data:
                        archive.addfile(info, data)
                else:
                    archive.addfile(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--revision", default="HEAD")
    parser.add_argument("--with-vcpkg", action="store_true")
    parser.add_argument("--corrosion-source", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    revision = git(root, "rev-parse", args.revision)
    epoch = int(git(root, "show", "-s", "--format=%ct", revision))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="sirius-source-", dir=args.output.parent
    ) as temporary:
        temporary = Path(temporary).resolve()
        source = temporary / "source"
        revisions = {}
        snapshot(root, revision, source, revisions, recurse=False)
        pins = submodules(root, revision)
        required = ["duckdb", "cucascade", "substrait"]
        if args.with_vcpkg:
            required.append("vcpkg")
        for path in required:
            snapshot(root / path, pins[path], source / path, revisions, path)
        vendor = source / "packaging/vendor"
        vendor.mkdir(parents=True, exist_ok=True)
        if args.with_vcpkg:
            if git(root / "vcpkg", "rev-parse", "HEAD") != pins["vcpkg"]:
                raise RuntimeError(
                    "Check out the pinned vcpkg submodule before packaging"
                )
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(root / "vcpkg"),
                    "-c",
                    "pack.threads=1",
                    "-c",
                    "pack.window=0",
                    "bundle",
                    "create",
                    str(vendor / "vcpkg.bundle"),
                    "HEAD",
                ],
                check=True,
            )

        corrosion = args.corrosion_source
        if corrosion is None:
            corrosion = temporary / "corrosion"
            subprocess.run(
                ["git", "init", str(corrosion)], check=True, stdout=subprocess.DEVNULL
            )
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(corrosion),
                    "fetch",
                    "--depth=1",
                    "https://github.com/corrosion-rs/corrosion.git",
                    CORROSION_REVISION,
                ],
                check=True,
            )
        snapshot(
            corrosion, CORROSION_REVISION, vendor / "corrosion", revisions, "corrosion"
        )
        download = temporary / "cuco.tar.gz"
        urllib.request.urlretrieve(
            f"https://github.com/NVIDIA/cuCollections/archive/{CUCO_REVISION}.tar.gz",
            download,
        )
        if checksum(download) != CUCO_SHA256:
            raise RuntimeError("cuCollections source checksum mismatch")
        with tarfile.open(download) as archive:
            archive.extractall(temporary / "cuco", filter="data")
        shutil.move(str(next((temporary / "cuco").iterdir())), vendor / "cuco")
        revisions["cuco"] = CUCO_REVISION
        config = subprocess.check_output(
            ["cargo", "vendor", "--locked", "--versioned-dirs", "vendor"],
            cwd=source / "rust",
        )
        (source / "rust/.cargo").mkdir(exist_ok=True)
        (source / "rust/.cargo/config.toml").write_bytes(config)
        manifest = {
            "revision": revision,
            "source_date_epoch": epoch,
            "revisions": revisions,
            "sha256": {
                name: checksum(source / name)
                for name in ("pixi.lock", "rust/Cargo.lock", "vcpkg.json")
            },
        }
        if args.with_vcpkg:
            manifest["sha256"]["packaging/vendor/vcpkg.bundle"] = checksum(
                vendor / "vcpkg.bundle"
            )
        (source / "packaging/input-manifest.json").write_text(
            json.dumps(manifest, indent=2) + "\n"
        )
        archive_tree(source, args.output, epoch)
    manifest["source_sha256"] = checksum(args.output)
    args.output.with_suffix(".json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"{manifest['source_sha256']}  {args.output}")


if __name__ == "__main__":
    main()
