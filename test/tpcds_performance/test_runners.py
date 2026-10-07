# =============================================================================
# Copyright 2026, Sirius Contributors.
#
# Licensed under the Apache License, Version 2.0 (the "License"); you may not use this file except
# in compliance with the License. You may obtain a copy of the License at
#
# http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software distributed under the License
# is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
# or implied. See the License for the specific language governing permissions and limitations under
# the License.
# =============================================================================

"""Run with: python -B -m unittest discover -s test/tpcds_performance -p 'test_*.py'."""

import csv
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class RunnerTests(unittest.TestCase):
    def test_parquet_inputs_and_process_status(self):
        for runner in ("duckdb", "super"):
            with self.subTest(runner=runner), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                scripts = root / "test/tpcds_performance"
                scripts.mkdir(parents=True)
                script = scripts / f"run_tpcds_{runner}.sh"
                shutil.copy(Path(__file__).with_name(script.name), script)
                queries = scripts / "queries"
                queries.mkdir()
                (queries / "q1.sql").write_text("SELECT 1;\n")
                data = root / "parquet"
                data.mkdir()
                for table in ("store", "store_sales", "customer", "customer_address"):
                    (data / f"{table}.parquet").touch()
                    (data / f"{table}_0.parquet").touch()
                    (data / f"{table}_10.parquet").touch()
                    (data / table).mkdir()
                    (data / table / "part0.parquet").touch()
                binary = root / "build/release/duckdb"
                binary.parent.mkdir(parents=True)
                binary.write_text(
                    "#!/usr/bin/env bash\n"
                    '[[ "$1" == "-bail" ]] || exit 99\n'
                    'cat > "$CAPTURE_SQL"\n'
                    'echo "error is valid result data"\n'
                    'echo "Run Time (s): real 0.01 user 0.01 sys 0.00"\n'
                    'echo "Run Time (s): real 0.02 user 0.01 sys 0.00"\n'
                    'exit "$FAKE_EXIT_STATUS"\n'
                )
                binary.chmod(0o755)
                config = root / "config.yaml"
                config.touch()
                capture = root / "input.sql"
                output = root / "output"
                args = ["bash", str(script)]
                if runner == "duckdb":
                    args.append("--parquet-dir")
                args += [str(data), "--queries", "1", "--output-dir", str(output)]
                for status in (0, 42):
                    result = subprocess.run(
                        args,
                        env=dict(
                            os.environ,
                            SIRIUS_CONFIG_FILE=str(config),
                            CAPTURE_SQL=str(capture),
                            FAKE_EXIT_STATUS=str(status),
                        ),
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, int(status != 0), result.stdout)
                    with (output / "timings.csv").open() as timings:
                        row = next(csv.DictReader(timings))
                    expected = "FAILED" if status else "OK"
                    self.assertEqual(row["run1_status"], expected)
                    self.assertEqual(row["run2_status"], expected)
                for table in ("store", "customer"):
                    view = next(
                        line
                        for line in capture.read_text().splitlines()
                        if line.startswith(f"CREATE VIEW {table} AS ")
                    )
                    for suffix in (
                        ".parquet",
                        "_0.parquet",
                        "_10.parquet",
                        "/part0.parquet",
                    ):
                        self.assertIn(f"'{data}/{table}{suffix}'", view)
                    self.assertNotIn("store_sales", view)
                    self.assertNotIn("customer_address", view)


if __name__ == "__main__":
    unittest.main()
