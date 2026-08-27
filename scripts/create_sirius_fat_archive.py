#!/usr/bin/env python3

import argparse
import shlex
import subprocess
from pathlib import Path


SYSTEM_LIBRARIES = {
    "c",
    "cuda",
    "dl",
    "gcc",
    "gcc_s",
    "m",
    "nvidia-ml",
    "pthread",
    "rt",
    "stdc++",
    "util",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Bundle the static inputs of the Sirius extension into one archive."
    )
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--ninja", type=Path, required=True)
    parser.add_argument("--ar", type=Path, required=True)
    parser.add_argument("--ranlib", type=Path, required=True)
    parser.add_argument("--source-archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dependencies-output", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--search-dir", type=Path, action="append", default=[])
    return parser.parse_args()


def extension_link_command(args: argparse.Namespace) -> list[str]:
    result = subprocess.run(
        [
            str(args.ninja),
            "-C",
            str(args.build_dir),
            "-t",
            "commands",
            "sirius_loadable_extension",
        ],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    candidates = [
        line
        for line in result.stdout.splitlines()
        if "sirius.duckdb_extension" in line and " -shared " in line
    ]
    if not candidates:
        raise RuntimeError(
            "could not locate the sirius_loadable_extension link command"
        )
    tokens = shlex.split(candidates[-1])
    if tokens[:2] == [":", "&&"]:
        tokens = tokens[2:]
    if "&&" in tokens:
        tokens = tokens[: tokens.index("&&")]
    return tokens


def resolve_path(token: str, build_dir: Path) -> Path | None:
    path = Path(token)
    if path.is_absolute():
        return path if path.is_file() else None
    candidate = build_dir / path
    return candidate if candidate.is_file() else None


def is_system_archive(path: Path) -> bool:
    if "sysroot" in path.parts or str(path).startswith("/usr/"):
        return True
    name = path.name
    return any(name == f"lib{library}.a" for library in SYSTEM_LIBRARIES)


def collect_archives(args: argparse.Namespace, tokens: list[str]) -> list[Path]:
    search_dirs = [directory.resolve() for directory in args.search_dir]
    for index, token in enumerate(tokens):
        if token == "-L" and index + 1 < len(tokens):
            search_dirs.append(Path(tokens[index + 1]).resolve())
        elif token.startswith("-L") and len(token) > 2:
            search_dirs.append(Path(token[2:]).resolve())

    archives = [args.source_archive.resolve()]
    unresolved: list[str] = []
    unexpected_shared: list[str] = []

    for token in tokens:
        if token.endswith(".a"):
            path = resolve_path(token, args.build_dir)
            if path is None:
                unresolved.append(token)
            elif not is_system_archive(path):
                archives.append(path.resolve())
        elif token.startswith("-l") and len(token) > 2:
            library = token[2:]
            if library in SYSTEM_LIBRARIES:
                continue
            match = next(
                (
                    directory / f"lib{library}.a"
                    for directory in search_dirs
                    if (directory / f"lib{library}.a").is_file()
                ),
                None,
            )
            if match is None:
                unresolved.append(token)
            else:
                archives.append(match.resolve())
        elif ".so" in token:
            path = resolve_path(token, args.build_dir)
            if path is None:
                continue
            allowed = (
                "sysroot" in path.parts
                or str(path).startswith("/usr/")
                or path.name.startswith("libcuda.so")
                or path.name.startswith("libnvidia-ml.so")
            )
            if not allowed:
                unexpected_shared.append(str(path))

    if unresolved:
        raise RuntimeError(
            "non-system static link inputs could not be resolved: "
            + ", ".join(sorted(set(unresolved)))
        )
    if unexpected_shared:
        raise RuntimeError(
            "vcpkg link unexpectedly contains shared user-space libraries: "
            + ", ".join(sorted(set(unexpected_shared)))
        )

    return list(dict.fromkeys(archives))


def merge_archives(output: Path, archives: list[Path], ar: Path, ranlib: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    commands = [f"create {output}"]
    commands.extend(f"addlib {archive}" for archive in archives)
    commands.extend(["save", "end"])
    subprocess.run(
        [str(ar), "-M"],
        check=True,
        input="\n".join(commands) + "\n",
        text=True,
    )

    ranlib_result = subprocess.run([str(ranlib), "-D", str(output)], check=False)
    if ranlib_result.returncode != 0:
        subprocess.run([str(ranlib), str(output)], check=True)


def create_archives(args: argparse.Namespace, archives: list[Path]) -> None:
    core_archives = [
        archive
        for archive in archives
        if archive == args.source_archive.resolve()
        or archive.name == "libdummy_static_extension_loader.a"
    ]
    dependency_archives = [
        archive for archive in archives if archive not in core_archives
    ]
    merge_archives(args.output, core_archives, args.ar, args.ranlib)
    merge_archives(args.dependencies_output, dependency_archives, args.ar, args.ranlib)

    members = subprocess.run(
        [str(args.ar), "t", str(args.output)],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout.splitlines()
    entry_member = "sirius_duckdb_entry.cpp.o"
    if entry_member in members:
        subprocess.run([str(args.ar), "d", str(args.output), entry_member], check=True)

    subprocess.run([str(args.ranlib), str(args.output)], check=True)

    manifest_entries = []
    for archive in archives:
        try:
            entry = archive.relative_to(args.build_dir.resolve())
        except ValueError:
            entry = Path(archive.name)
        manifest_entries.append(entry.as_posix())
    args.manifest.write_text("\n".join(manifest_entries) + "\n", encoding="utf-8")


def main() -> None:
    args = parse_args()
    tokens = extension_link_command(args)
    archives = collect_archives(args, tokens)
    create_archives(args, archives)


if __name__ == "__main__":
    main()
