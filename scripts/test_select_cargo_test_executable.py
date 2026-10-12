import contextlib
import importlib.util
import io
import json
import os
import tempfile
import unittest
from unittest import mock
from pathlib import Path


SCRIPT = Path(__file__).with_name("select-cargo-test-executable.py")
SPEC = importlib.util.spec_from_file_location("cargo_test_executable", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MODULE)


def artifact(*, name="qc_remote_lib", kind=("rlib",), crate_types=("rlib",), test=True, executable="/tmp/test-bin"):
    return {
        "reason": "compiler-artifact",
        "target": {"name": name, "kind": list(kind), "crate_types": list(crate_types)},
        "profile": {"test": test},
        "executable": executable,
    }


class SelectCargoTestExecutableTests(unittest.TestCase):
    def test_selects_exact_library_test_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "qc_remote_lib-test"
            path.touch()
            selected = artifact(executable=str(path))
            self.assertEqual(
                MODULE.select_executable([{"reason": "build-finished"}, selected]),
                str(path),
            )

    def test_rejects_missing_and_ambiguous_artifacts(self):
        with self.assertRaisesRegex(ValueError, "found 0"):
            MODULE.select_executable([])
        with self.assertRaisesRegex(ValueError, "found 2"):
            MODULE.select_executable([artifact(), artifact(executable="/tmp/second")])

    def test_rejects_wrong_target_kind_or_non_test_artifacts(self):
        for item in (
            artifact(name="qc_device_broker"),
            artifact(kind=("bin",), crate_types=("bin",)),
            artifact(test=False),
        ):
            with self.subTest(item=item), self.assertRaises(ValueError):
                MODULE.select_executable([item])

    def test_rejects_missing_or_relative_executable_paths(self):
        with self.assertRaisesRegex(ValueError, "existing absolute file"):
            MODULE.select_executable([artifact(executable="relative-test-bin")])

    def test_rejects_environment_file_injection(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "test-bin"
            executable.touch()
            with self.assertRaisesRegex(ValueError, "line break"):
                MODULE.select_executable([artifact(executable=f"{executable}\nMALICIOUS=value")])

    def test_environment_write_failure_is_reported(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "test-bin"
            executable.touch()
            record = json.dumps(artifact(executable=str(executable))) + "\n"
            with (
                mock.patch.dict(os.environ, {"GITHUB_ENV": directory}),
                mock.patch.object(MODULE.sys, "stdin", io.StringIO(record)),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(MODULE.main(), 1)


if __name__ == "__main__":
    unittest.main()
