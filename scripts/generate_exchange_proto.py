#!/usr/bin/env python3
"""Regenerate exchange C++ metadata with the bundled protobuf runtime's compiler.

Run: pixi exec --spec protobuf=3.19.4 -- python scripts/generate_exchange_proto.py
"""

from pathlib import Path
import subprocess


def main() -> None:
    version = subprocess.check_output(["protoc", "--version"], text=True).strip()
    if version != "libprotoc 3.19.4":
        raise SystemExit(f"Expected libprotoc 3.19.4, got {version}")
    root = Path(__file__).resolve().parent.parent
    output = root / "src/exchange/generated"
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "protoc",
            f"--proto_path={root / 'proto'}",
            f"--cpp_out={output}",
            str(root / "proto/sirius/exchange/v1/exchange.proto"),
        ],
        check=True,
    )


if __name__ == "__main__":
    main()
