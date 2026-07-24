"""Isolated DuckDB worker used by sirius-runner.

The Rust process owns orchestration and files. This process owns exactly one
DuckDB connection for one engine/query trial and communicates through JSON
files so benchmark output never mixes with the CLI's stdout.
"""

from __future__ import annotations

import datetime
import decimal
import hashlib
import importlib
import importlib.util
import json
import math
import os
import sys
import threading
import time
import traceback
import uuid
from pathlib import Path
from typing import Any


PROTOCOL_VERSION = 2
TPCH_TABLES = (
    "customer",
    "lineitem",
    "nation",
    "orders",
    "part",
    "partsupp",
    "region",
    "supplier",
)


def log(message: str) -> None:
    timestamp = datetime.datetime.now().strftime("%H:%M:%S")
    line = f"[{timestamp}] worker: {message}"
    print(line, file=sys.stderr, flush=True)
    if log_path := os.environ.get("SIRIUS_WORKER_LOG"):
        with open(log_path, "a", encoding="utf-8") as handle:
            print(line, file=handle)


def sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


SIGNED_INTEGER_TYPES = frozenset(
    {"tinyint", "smallint", "integer", "bigint", "hugeint"}
)
UNSIGNED_INTEGER_TYPES = frozenset(
    {"utinyint", "usmallint", "uinteger", "ubigint", "uhugeint"}
)
FLOAT_TYPES = frozenset({"float", "double"})
TEXT_TYPES = frozenset({"varchar", "enum", "bit"})
TIMESTAMP_TYPES = frozenset(
    {"timestamp", "timestamp_s", "timestamp_ms", "timestamp_tz"}
)
TYPE_ID_ALIASES = {
    "time with time zone": "time_tz",
    "timestamp with time zone": "timestamp_tz",
}


class ResultEncodingError(TypeError):
    pass


def logical_type_id(logical_type: Any) -> str:
    try:
        raw_id = str(logical_type.id).lower()
        return TYPE_ID_ALIASES.get(raw_id, raw_id)
    except Exception as error:
        raise ResultEncodingError(
            f"could not inspect DuckDB logical type {logical_type!s}"
        ) from error


def logical_type_children(logical_type: Any) -> list[tuple[str, Any]]:
    try:
        return list(logical_type.children)
    except Exception as error:
        raise ResultEncodingError(
            f"could not inspect nested DuckDB logical type {logical_type!s}"
        ) from error


def expect_python_type(
    value: Any, expected: type | tuple[type, ...], logical_type: Any
) -> None:
    if not isinstance(value, expected):
        raise ResultEncodingError(
            f"DuckDB {logical_type!s} returned unsupported Python value "
            f"{type(value).__module__}.{type(value).__qualname__}"
        )


def typed_value(value: Any, logical_type: Any) -> dict[str, Any]:
    if value is None:
        return {"type": "null"}

    type_id = logical_type_id(logical_type)
    logical_name = str(logical_type).upper()

    if type_id == "boolean":
        expect_python_type(value, bool, logical_type)
        return {"type": "boolean", "value": value}
    if type_id in SIGNED_INTEGER_TYPES:
        if isinstance(value, bool) or not isinstance(value, int):
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} returned unsupported Python value "
                f"{type(value).__module__}.{type(value).__qualname__}"
            )
        return {"type": "integer", "value": str(value)}
    if type_id in UNSIGNED_INTEGER_TYPES:
        if isinstance(value, bool) or not isinstance(value, int) or value < 0:
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} returned invalid unsigned value {value!r}"
            )
        return {"type": "unsigned_integer", "value": str(value)}
    if type_id == "bignum":
        expect_python_type(value, str, logical_type)
        return {"type": "integer", "value": value}
    if type_id in FLOAT_TYPES:
        expect_python_type(value, float, logical_type)
        if math.isnan(value):
            encoded = "nan"
        elif math.isinf(value):
            encoded = "inf" if value > 0 else "-inf"
        else:
            encoded = repr(value)
        return {"type": "float", "value": encoded}
    if type_id == "decimal":
        expect_python_type(value, decimal.Decimal, logical_type)
        return {"type": "decimal", "value": str(value)}
    if type_id in TIMESTAMP_TYPES:
        expect_python_type(value, datetime.datetime, logical_type)
        without_timezone = value.replace(tzinfo=None)
        if without_timezone in (datetime.datetime.min, datetime.datetime.max):
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} boundary and infinity values are "
                "ambiguous in the Python API"
            )
        return {"type": "timestamp", "value": value.isoformat()}
    if type_id == "timestamp_ns":
        raise ResultEncodingError(
            "DuckDB TIMESTAMP_NS is unsupported because the Python API truncates "
            "nanoseconds to microseconds"
        )
    if type_id == "date":
        expect_python_type(value, datetime.date, logical_type)
        if value in (datetime.date.min, datetime.date.max):
            raise ResultEncodingError(
                "DuckDB DATE boundary and infinity values are ambiguous in the Python API"
            )
        return {"type": "date", "value": value.isoformat()}
    if type_id in {"time", "time_tz"}:
        expect_python_type(value, datetime.time, logical_type)
        return {"type": "time", "value": value.isoformat()}
    if type_id == "time_ns":
        raise ResultEncodingError(
            "DuckDB TIME_NS is unsupported because the Python API truncates "
            "nanoseconds to microseconds"
        )
    if type_id == "interval":
        raise ResultEncodingError(
            "DuckDB INTERVAL is unsupported because the Python API collapses "
            "months and days into datetime.timedelta"
        )
    if type_id == "blob":
        expect_python_type(value, (bytes, bytearray, memoryview), logical_type)
        return {"type": "blob", "value": bytes(value).hex()}
    if type_id == "uuid":
        expect_python_type(value, uuid.UUID, logical_type)
        return {"type": "uuid", "value": str(value)}
    if logical_name == "JSON":
        expect_python_type(value, str, logical_type)
        try:
            json.loads(value)
        except (TypeError, ValueError) as error:
            raise ResultEncodingError("DuckDB JSON returned invalid JSON text") from error
        return {"type": "json", "value": value}
    if type_id in TEXT_TYPES:
        expect_python_type(value, str, logical_type)
        return {"type": "text", "value": value}
    if type_id in {"list", "array"}:
        expect_python_type(value, (list, tuple), logical_type)
        children = logical_type_children(logical_type)
        if not children:
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} did not expose its child type"
            )
        child_type = children[0][1]
        if type_id == "array":
            if len(children) != 2 or children[1][0] != "size":
                raise ResultEncodingError(
                    f"DuckDB {logical_type!s} exposed an unexpected ARRAY shape"
                )
            if len(value) != children[1][1]:
                raise ResultEncodingError(
                    f"DuckDB {logical_type!s} returned {len(value)} values"
                )
        return {
            "type": "list",
            "value": [typed_value(item, child_type) for item in value],
        }
    if type_id == "struct":
        expect_python_type(value, dict, logical_type)
        children = logical_type_children(logical_type)
        child_names = [name for name, _ in children]
        if set(value) != set(child_names):
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} returned fields {list(value)!r}, "
                f"expected {child_names!r}"
            )
        fields = {
            name: typed_value(value[name], child_type)
            for name, child_type in children
        }
        return {"type": "struct", "value": fields}
    if type_id == "map":
        expect_python_type(value, dict, logical_type)
        children = logical_type_children(logical_type)
        if len(children) != 2 or [name for name, _ in children] != ["key", "value"]:
            raise ResultEncodingError(
                f"DuckDB {logical_type!s} exposed an unexpected MAP shape"
            )
        key_type, value_type = children[0][1], children[1][1]
        entries = [
            {
                "key": typed_value(key, key_type),
                "value": typed_value(field, value_type),
            }
            for key, field in value.items()
        ]
        return {"type": "map", "value": entries}

    raise ResultEncodingError(
        f"unsupported DuckDB logical type {logical_type!s} "
        f"(Python value {type(value).__module__}.{type(value).__qualname__})"
    )


def encode_result(cursor: Any, rows: list[tuple[Any, ...]]) -> dict[str, Any]:
    description = cursor.description
    schema = {
        "columns": [
            {"name": str(column[0]), "logical_type": str(column[1])}
            for column in description
        ]
    }
    encoded_rows = []
    for row_index, row in enumerate(rows):
        encoded_row = []
        for column_index, (value, column) in enumerate(zip(row, description, strict=True)):
            try:
                encoded_row.append(typed_value(value, column[1]))
            except ResultEncodingError as error:
                raise ResultEncodingError(
                    f"could not encode row {row_index + 1}, column "
                    f"{column_index + 1} ({column[0]!r} {column[1]!s}): {error}"
                ) from error
        encoded_rows.append(encoded_row)
    return {
        "schema": schema,
        "rows": encoded_rows,
        "row_count": len(encoded_rows),
    }


def sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def parquet_files(root: Path, table: str) -> list[Path]:
    candidates: list[Path] = []
    for pattern in (f"{table}.parquet", f"{table}_*.parquet", f"{table}/*.parquet"):
        candidates.extend(sorted(root.glob(pattern)))
    return list(dict.fromkeys(path.resolve() for path in candidates if path.is_file()))


def register_parquet_views(
    connection: Any, root: Path, deadline: TrialDeadline
) -> None:
    log(f"registering TPC-H parquet views from {root}")
    for table in TPCH_TABLES:
        deadline.raise_if_expired()
        files = parquet_files(root, table)
        if not files:
            raise FileNotFoundError(f"no parquet files found for table {table!r} in {root}")
        paths = ", ".join(sql_string(str(path)) for path in files)
        execute_with_deadline(
            connection,
            f'CREATE VIEW "{table}" AS SELECT * FROM read_parquet([{paths}])',
            deadline,
        )


class QueryTimeout(TimeoutError):
    pass


class TrialDeadline:
    def __init__(self, label: str, timeout_s: int) -> None:
        self.label = label
        self.timeout_s = timeout_s
        self._finished = threading.Event()
        self._expired = threading.Event()
        self._connection_lock = threading.Lock()
        self._connection: Any | None = None
        self._watchdog = threading.Thread(
            target=self._watch,
            name="sirius-runner-deadline",
            daemon=True,
        )

    @property
    def expired(self) -> bool:
        return self._expired.is_set()

    def start(self) -> None:
        self._watchdog.start()

    def attach_connection(self, connection: Any) -> None:
        with self._connection_lock:
            self._connection = connection
        self.raise_if_expired()

    def detach_connection(self, connection: Any) -> None:
        with self._connection_lock:
            if self._connection is connection:
                self._connection = None

    def stop(self) -> None:
        self._finished.set()
        self._watchdog.join(timeout=1.0)

    def raise_if_expired(self) -> None:
        if self.expired:
            raise QueryTimeout(
                f"{self.label} trial exceeded its {self.timeout_s}s timeout"
            )

    def _watch(self) -> None:
        if self._finished.wait(self.timeout_s):
            return
        self._expired.set()
        log(
            f"{self.label}: trial deadline reached after {self.timeout_s}s; "
            "interrupting DuckDB"
        )
        with self._connection_lock:
            if self._connection is not None:
                try:
                    self._connection.interrupt()
                except Exception as error:
                    log(f"{self.label}: DuckDB interrupt failed: {error}")


def execute_with_deadline(
    connection: Any, sql: str, deadline: TrialDeadline
) -> tuple[Any, list]:
    deadline.raise_if_expired()
    try:
        cursor = connection.execute(sql)
        rows = cursor.fetchall()
    except Exception as error:
        if deadline.expired:
            raise QueryTimeout(
                f"{deadline.label} trial exceeded its {deadline.timeout_s}s timeout"
            ) from error
        raise
    deadline.raise_if_expired()
    return cursor, rows


def identity() -> dict[str, Any]:
    duckdb = importlib.import_module("duckdb")
    native = importlib.import_module("_duckdb")
    module_path = str(Path(native.__file__).resolve())
    duckdb_threads = available_cpu_count()
    return {
        "schema_version": PROTOCOL_VERSION,
        "duckdb_version": duckdb.__version__,
        "duckdb_threads": duckdb_threads,
        "preserve_insertion_order": True,
        "module_path": module_path,
        "module_sha256": sha256_file(module_path),
        "python_version": sys.version.split()[0],
        "python_executable": str(Path(sys.executable).resolve()),
    }


def available_cpu_count() -> int:
    try:
        count = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        count = os.cpu_count()
    return max(1, int(count or 1))


def requested_execution_settings(request: dict[str, Any]) -> tuple[int, bool]:
    duckdb_threads = request["duckdb_threads"]
    if isinstance(duckdb_threads, bool) or not isinstance(duckdb_threads, int):
        raise ValueError("duckdb_threads must be an integer")
    if duckdb_threads < 1:
        raise ValueError("duckdb_threads must be greater than zero")

    preserve_insertion_order = request["preserve_insertion_order"]
    if preserve_insertion_order is not True:
        raise ValueError("preserve_insertion_order must be true")
    return duckdb_threads, preserve_insertion_order


def apply_execution_settings(
    connection: Any,
    request: dict[str, Any],
    deadline: TrialDeadline,
) -> tuple[int, bool]:
    duckdb_threads, preserve_insertion_order = requested_execution_settings(request)
    execute_with_deadline(connection, f"SET threads = {duckdb_threads}", deadline)
    execute_with_deadline(
        connection,
        "SET preserve_insertion_order = true",
        deadline,
    )
    _, rows = execute_with_deadline(
        connection,
        "SELECT current_setting('threads')::INTEGER, "
        "current_setting('preserve_insertion_order')::BOOLEAN",
        deadline,
    )
    applied = (duckdb_threads, preserve_insertion_order)
    if rows != [applied]:
        raise RuntimeError(
            f"DuckDB applied execution settings {rows!r}, expected {[applied]!r}"
        )
    return applied


def open_connection(request: dict[str, Any], deadline: TrialDeadline) -> Any:
    duckdb = importlib.import_module("duckdb")
    source = request["source"]
    if source["format"] == "duckdb":
        path = str(Path(source["path"]).resolve())
        log(f"opening DuckDB database {path} read-only")
        connection = duckdb.connect(
            path, read_only=True, config={"allow_unsigned_extensions": "true"}
        )
    elif source["format"] == "parquet":
        connection = duckdb.connect(
            ":memory:", config={"allow_unsigned_extensions": "true"}
        )
    elif source["format"] == "none":
        connection = duckdb.connect(
            ":memory:", config={"allow_unsigned_extensions": "true"}
        )
    else:
        raise ValueError(f"unsupported data source format: {source['format']}")

    try:
        deadline.attach_connection(connection)
        apply_execution_settings(connection, request, deadline)
        execute_with_deadline(connection, "SET TimeZone = 'UTC'", deadline)
        if source["format"] == "parquet":
            register_parquet_views(connection, Path(source["path"]).resolve(), deadline)
        if request["engine"] == "sirius":
            extension = str(Path(request["extension_path"]).resolve())
            log(f"loading Sirius extension {extension}")
            execute_with_deadline(
                connection, f"LOAD {sql_string(extension)}", deadline
            )
            execute_with_deadline(
                connection, "SET enable_duckdb_fallback = false", deadline
            )
            execute_with_deadline(connection, "SET gpu_execution = true", deadline)
        elif request["engine"] != "duckdb":
            raise ValueError(f"unsupported engine: {request['engine']}")
    except Exception:
        deadline.detach_connection(connection)
        connection.close()
        raise

    return connection


def execute_multi(connection: Any, sql: str, deadline: TrialDeadline) -> None:
    for statement in sql.split(";"):
        statement = statement.strip()
        if statement:
            execute_with_deadline(connection, statement, deadline)


def pin_helpers(repo_root: str) -> Any:
    path = Path(repo_root) / "test/tpch_performance/tpch_pin_columns.py"
    spec = importlib.util.spec_from_file_location("sirius_tpch_pin_columns", path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load pin helpers from {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run_trial(request: dict[str, Any]) -> dict[str, Any]:
    query = request["query"]
    engine = request["engine"]
    warmups = int(request["warmups"])
    iterations = int(request["iterations"])
    timeout_s = int(request["timeout_s"])
    duckdb_threads, preserve_insertion_order = requested_execution_settings(request)
    if warmups < 0 or iterations < 1 or timeout_s < 1:
        raise ValueError("warmups must be non-negative; iterations and timeout_s must be positive")

    log(
        f"starting {engine}/{query} ({warmups} warm-up, {iterations} measured; "
        f"{timeout_s}s total trial timeout; {duckdb_threads} DuckDB threads)"
    )
    deadline = TrialDeadline(f"{engine}/{query}", timeout_s)
    deadline.start()
    connection = None
    pin = request.get("pin", "none")
    pinned = engine == "sirius" and pin != "none"
    helpers = None
    try:
        connection = open_connection(request, deadline)
        if pinned:
            os.environ["SIRIUS_PIN_TIER"] = pin
            helpers = pin_helpers(request["repo_root"])
            query_number = int(query.removeprefix("q"))
            log(f"{engine}/{query}: pinning the {pin} working set")
            execute_multi(
                connection,
                helpers.emit_pin(
                    query_number, request["source"]["path"], request["source"]["format"]
                ),
                deadline,
            )
        for index in range(warmups):
            log(f"{engine}/{query}: warm-up {index + 1}/{warmups}")
            execute_with_deadline(connection, request["sql"], deadline)

        measurements = []
        for index in range(iterations):
            log(f"{engine}/{query}: measured iteration {index + 1}/{iterations}")
            started = time.perf_counter_ns()
            cursor, rows = execute_with_deadline(connection, request["sql"], deadline)
            duration_ns = time.perf_counter_ns() - started
            encoded_result = encode_result(cursor, rows)
            deadline.raise_if_expired()
            measurements.append(
                {
                    "iteration": index + 1,
                    "duration_ns": duration_ns,
                    "result": encoded_result,
                }
            )
            log(
                f"{engine}/{query}: iteration {index + 1} completed "
                f"in {duration_ns / 1_000_000_000:.3f}s"
            )
    finally:
        try:
            if connection is not None and pinned and helpers is not None:
                if deadline.expired:
                    log(f"{engine}/{query}: skipping unpin after trial timeout")
                else:
                    log(f"{engine}/{query}: unpinning working set")
                    execute_multi(
                        connection,
                        helpers.emit_unpin(int(query.removeprefix("q"))),
                        deadline,
                    )
        finally:
            if connection is not None:
                deadline.detach_connection(connection)
                connection.close()
            deadline.stop()

    return {
        "schema_version": PROTOCOL_VERSION,
        "engine": engine,
        "query": query,
        "duckdb_threads": duckdb_threads,
        "preserve_insertion_order": preserve_insertion_order,
        "fallback_disabled": engine == "sirius",
        "pin": pin,
        "pin_setup_succeeded": pinned,
        "warmups": warmups,
        "measurements": measurements,
    }


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: worker.py REQUEST.json RESPONSE.json", file=sys.stderr)
        return 2

    request_path = Path(sys.argv[1])
    response_path = Path(sys.argv[2])
    try:
        with request_path.open(encoding="utf-8") as handle:
            request = json.load(handle)
        if request.get("schema_version") != PROTOCOL_VERSION:
            raise ValueError(
                f"unsupported worker protocol {request.get('schema_version')!r}; "
                f"expected {PROTOCOL_VERSION}"
            )
        if request["operation"] == "identity":
            response = identity()
        elif request["operation"] == "trial":
            response = run_trial(request)
        else:
            raise ValueError(f"unsupported worker operation: {request['operation']!r}")
        response_path.write_text(
            json.dumps(response, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        return 0
    except Exception as error:
        log(f"failed: {error}")
        traceback.print_exc(file=sys.stderr)
        failure = {
            "schema_version": PROTOCOL_VERSION,
            "error": str(error),
            "error_type": type(error).__name__,
        }
        try:
            response_path.write_text(
                json.dumps(failure, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        except OSError:
            pass
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
