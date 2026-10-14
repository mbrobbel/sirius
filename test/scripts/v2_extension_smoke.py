#!/usr/bin/env python3
"""Smoke-test native and Parquet GPU queries through a packaged Sirius extension."""

import argparse
from pathlib import Path
import subprocess
import sys
import tempfile


def literal(value):
    return "'" + str(value).replace("'", "''") + "'"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duckdb", type=Path, required=True)
    parser.add_argument("--extension", type=Path, required=True)
    parser.add_argument(
        "--backend",
        type=Path,
        action="append",
        default=[],
        help="Install this backend before testing the CUDA-selecting loader (repeat for each backend)",
    )
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="sirius-v2-smoke-") as directory:
        scratch = Path(directory)
        parquet = scratch / "numbers.parquet"
        load = f"LOAD {literal(args.extension.resolve())};"
        if args.backend:
            installs = "\n".join(
                f"INSTALL {literal(path.resolve())};" for path in args.backend
            )
            load = f"""
SET extension_directories=[{literal(scratch / 'extensions')}];
SET autoinstall_known_extensions=false;
{installs}
INSTALL {literal(args.extension.resolve())};
LOAD sirius;
SELECT 'SIRIUS_BACKENDS=' || count(*) FROM duckdb_extensions()
WHERE extension_name IN ('sirius_cuda12', 'sirius_cuda13') AND loaded;
"""
        sql = f"""
.bail on
.mode list
.headers off
{load}
SET sirius_log_backend = 'duckdb';
CALL enable_logging(storage='stdout');
SET gpu_execution = false;
CREATE TABLE numbers AS SELECT i::BIGINT AS id FROM range(100000) t(i);
CHECKPOINT;
COPY numbers TO {literal(parquet)} (FORMAT PARQUET, COMPRESSION ZSTD);
SET enable_duckdb_fallback = false;
SET gpu_execution = true;
SELECT SUM(id) FROM numbers WHERE id > 10;
SELECT SUM(id) FROM read_parquet({literal(parquet)}) WHERE id > 10;
"""
        print(sql, file=sys.stderr, flush=True)
        results = completed = backend_checks = 0
        # Check known sums and GPU completions; forward diagnostics as they arrive.
        expected = str(sum(range(11, 100000)))
        with subprocess.Popen(
            [
                str(args.duckdb.resolve()),
                "-unsigned",
                "-no-agent",
                str(scratch / "smoke.db"),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        ) as process:
            try:
                process.stdin.write(sql)
                process.stdin.close()
            except BrokenPipeError:
                pass
            for line in process.stdout:
                print(line, end="", file=sys.stderr, flush=True)
                backend_checks += line.strip() == "SIRIUS_BACKENDS=1"
                results += line.strip() == expected
                completed += "Transparent GPU execution: query completed" in line
            if process.wait():
                raise RuntimeError(f"DuckDB exited {process.returncode}")
        if args.backend and backend_checks != 1:
            raise AssertionError("The loader must load exactly one CUDA backend")
        if results != 2 or completed != 2:
            raise AssertionError(
                f"Expected 2 correct results and 2 GPU executions; got {results} and {completed}"
            )
        print("Passed native and Parquet GPU smoke checks", file=sys.stderr)


if __name__ == "__main__":
    main()
