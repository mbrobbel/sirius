# Copyright 2026, Sirius Contributors. SPDX-License-Identifier: Apache-2.0
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "source_inputs", ROOT / "scripts/prepare-package-source.py"
)
SOURCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SOURCE)


class SourceInputTests(unittest.TestCase):
    def test_deterministic_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            source = directory / "source"
            source.mkdir()
            (source / "file with spaces").write_text("content")
            (source / "alias").symlink_to("file with spaces")
            first, second = directory / "first.tar.gz", directory / "second.tar.gz"
            SOURCE.archive_tree(source, first, 1234567890)
            SOURCE.archive_tree(source, second, 1234567890)
            self.assertEqual(first.read_bytes(), second.read_bytes())

    def test_uninitialized_submodule_does_not_use_parent_repository(self):
        with tempfile.TemporaryDirectory() as temporary:
            repository = Path(temporary)
            subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
            child = repository / "uninitialized"
            child.mkdir()
            with self.assertRaisesRegex(RuntimeError, "Initialize the submodule"):
                SOURCE.snapshot(child, "HEAD", repository / "output", {})


if __name__ == "__main__":
    unittest.main()
