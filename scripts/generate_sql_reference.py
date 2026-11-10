#!/usr/bin/env python3
# Copyright 2026, Sirius Contributors.
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
"""Generate docs/sql-reference.md from the metadata a built Sirius extension registers.

Loads the extension with SIRIUS_DISABLE=1, so no GPU is needed, and renders its table
functions (duckdb_functions()) and settings (duckdb_settings()).
"""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_DUCKDB = ROOT / "sirius-duckdb/build/release/duckdb"
DEFAULT_EXTENSION = (
    ROOT / "sirius-duckdb/build/release/extension/sirius/sirius.duckdb_extension"
)

SETTINGS_SQL = """
CREATE TEMP TABLE builtin AS SELECT name FROM duckdb_settings(include_all := true);
LOAD {extension};
SELECT s.name, s.input_type, s.description,
       s.name IN (SELECT name FROM duckdb_settings(debug := true)) AS tuning,
       s.name IN (SELECT name FROM duckdb_settings(deprecated := true)) AS deprecated
FROM duckdb_settings(include_all := true) s
WHERE s.name NOT IN (SELECT name FROM builtin)
ORDER BY s.name;
"""

FUNCTIONS_SQL = """
LOAD {extension};
SELECT function_name, parameters, parameter_types, description, examples, categories
FROM duckdb_functions()
WHERE extension_name IN (SELECT extension_name FROM duckdb_extensions() WHERE loaded)
  AND extension_name LIKE 'sirius%'
ORDER BY function_name, len(parameters) DESC;
"""

CATEGORY_TITLES = {
    "execution": "Execution",
    "cache": "Pinned tables and caches",
    "vector_search": "Vector search",
    "diagnostics": "Diagnostics",
    "metadata": "Metadata",
    "telemetry": "Telemetry",
    "developer": "Developer",
    "internal": "Internal",
}


def query(duckdb, sql):
    environment = dict(os.environ, SIRIUS_DISABLE="1", SIRIUS_LOG_BACKEND="noop")
    result = subprocess.run(
        [str(duckdb), "-unsigned", "-json", "-c", sql],
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        sys.exit(f"duckdb failed:\n{result.stderr}")
    # One JSON array per statement that returns rows; the last one holds the result.
    rows = []
    decoder = json.JSONDecoder()
    text = result.stdout.strip()
    while text:
        value, end = decoder.raw_decode(text)
        rows = value
        text = text[end:].strip()
    return rows


def sql_string(value):
    return "'" + value.replace("'", "''") + "'"


def markdown_text(text):
    return (text or "").replace("\\", "\\\\")


def table_cell(text):
    return markdown_text(text).replace("|", "\\|").replace("\n", " ")


def render_functions(rows):
    sections = {}
    for row in rows:
        category = (row["categories"] or ["other"])[0]
        sections.setdefault(category, []).append(row)
    order = [c for c in CATEGORY_TITLES if c in sections]
    order += sorted(c for c in sections if c not in CATEGORY_TITLES)
    lines = ["## Table functions", ""]
    for category in order:
        lines += [f"### {CATEGORY_TITLES.get(category, category.capitalize())}", ""]
        for row in sections[category]:
            parameters = ", ".join(
                f"{name} {kind}"
                for name, kind in zip(row["parameters"], row["parameter_types"])
            )
            lines += [f"#### `{row['function_name']}({parameters})`", ""]
            lines += [markdown_text(row["description"]) or "_No description._", ""]
            if row["examples"]:
                lines += ["```sql", *row["examples"], "```", ""]
    return lines


def render_settings(rows):
    groups = [
        (
            "Settings",
            "Set with `SET name = value;`.",
            lambda r: not r["tuning"] and not r["deprecated"],
        ),
        (
            "Tuning settings",
            "Low-level knobs, hidden from `duckdb_settings()` unless called with `debug := true`; "
            "they can still be set.",
            lambda r: r["tuning"],
        ),
        ("Deprecated settings", "", lambda r: r["deprecated"]),
    ]
    lines = []
    for title, intro, keep in groups:
        selected = [r for r in rows if keep(r)]
        if not selected:
            continue
        lines += [f"## {title}", ""]
        if intro:
            lines += [intro, ""]
        lines += ["| Setting | Type | Description |", "| --- | --- | --- |"]
        lines += [
            f"| `{r['name']}` | {r['input_type']} | {table_cell(r['description'])} |"
            for r in selected
        ]
        lines.append("")
    return lines


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--duckdb", type=Path, default=DEFAULT_DUCKDB)
    parser.add_argument("--extension", type=Path, default=DEFAULT_EXTENSION)
    parser.add_argument(
        "-o", "--output", type=Path, default=ROOT / "docs/sql-reference.md"
    )
    args = parser.parse_args()
    extension = sql_string(str(args.extension.resolve()))
    functions = query(args.duckdb, FUNCTIONS_SQL.format(extension=extension))
    settings = query(args.duckdb, SETTINGS_SQL.format(extension=extension))
    if not functions or not settings:
        sys.exit("the extension registered no functions or settings")
    lines = [
        "# Sirius SQL reference",
        "",
        "<!-- Generated by scripts/generate_sql_reference.py from a Sirius build; do not edit. -->",
        "",
        "Table functions and settings the Sirius CUDA backend registers, as `duckdb_functions()` "
        "and `duckdb_settings()` report them. The `sirius` loader adds `sirius_status()`, "
        "described in [sirius-duckdb/README.md](../sirius-duckdb/README.md). Some settings also "
        "have `sirius.yaml` counterparts; see the "
        "[configuration reference](super-sirius/configuration.md).",
        "",
        *render_functions(functions),
        *render_settings(settings),
    ]
    args.output.write_text("\n".join(lines).rstrip() + "\n")
    print(f"wrote {args.output}: {len(functions)} functions, {len(settings)} settings")


if __name__ == "__main__":
    main()
