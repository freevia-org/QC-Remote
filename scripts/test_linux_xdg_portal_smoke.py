"""Fast, display-independent checks for the Linux portal smoke driver."""

from __future__ import annotations

import json
import contextlib
import io
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import scripts.run_linux_xdg_portal_smoke as runner
from scripts.run_linux_xdg_portal_smoke import (
    require_portal_calls,
    select_executable,
    stop_owned_process,
)


class CargoTestExecutableSelectionTests(unittest.TestCase):
    def write_messages(self, root: Path, messages: list[dict[str, object]]) -> tuple[Path, Path]:
        executable = root / "linux_xdg_portal_smoke-test"
        executable.write_text("test executable fixture", encoding="utf-8")
        cargo_json = root / "cargo.jsonl"
        cargo_json.write_text("\n".join(json.dumps(message) for message in messages), encoding="utf-8")
        return cargo_json, executable

    def candidate(self, executable: Path) -> dict[str, object]:
        return {
            "reason": "compiler-artifact",
            "target": {"name": "linux_xdg_portal_smoke", "kind": ["test"]},
            "profile": {"test": True},
            "executable": str(executable),
        }

    def test_selects_only_the_named_integration_test_executable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo_json, expected = self.write_messages(
                root,
                [
                    {"reason": "compiler-artifact", "target": {"name": "qc-remote", "kind": ["bin"]}},
                    self.candidate(expected := root / "linux_xdg_portal_smoke-test"),
                ],
            )
            self.assertEqual(select_executable(cargo_json), expected)

    def test_rejects_missing_or_ambiguous_test_executables(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo_json, executable = self.write_messages(root, [])
            with self.assertRaisesRegex(ValueError, "found 0"):
                select_executable(cargo_json)
            cargo_json.write_text(
                json.dumps(self.candidate(executable)) + "\n" + json.dumps(self.candidate(executable)),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "found 2"):
                select_executable(cargo_json)

    def test_rejects_a_test_artifact_path_that_does_not_exist(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo_json = root / "cargo.jsonl"
            cargo_json.write_text(json.dumps(self.candidate(root / "missing-test-bin")), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "existing absolute file"):
                select_executable(cargo_json)


class PortalRouteAssertionTests(unittest.TestCase):
    def test_monitor_readiness_accepts_the_initial_dbus_name_acquired_signal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / "dbus-monitor.log"
            log.write_text(
                "signal time=1 sender=org.freedesktop.DBus "
                "interface=org.freedesktop.DBus; member=NameAcquired\n",
                encoding="utf-8",
            )
            monitor = mock.Mock()
            monitor.poll.return_value = None
            runner.wait_for_monitor_ready(monitor, log, timeout_seconds=0.1)

    def test_monitor_readiness_times_out_without_a_connected_monitor_marker(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / "dbus-monitor.log"
            log.write_text("", encoding="utf-8")
            monitor = mock.Mock()
            monitor.poll.return_value = None
            with self.assertRaisesRegex(TimeoutError, "did not become ready"):
                runner.wait_for_monitor_ready(monitor, log, timeout_seconds=0)

    def test_failure_diagnostics_emit_bounded_log_tails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "portal-test.log").write_text(
                "\n".join(f"test line {number}" for number in range(100)), encoding="utf-8"
            )
            (root / "dbus-monitor.log").write_text("dbus diagnostic\n", encoding="utf-8")
            stderr = io.StringIO()
            with contextlib.redirect_stderr(stderr):
                runner.emit_failure_diagnostics(root)

        diagnostics = stderr.getvalue()
        self.assertIn("test line 99", diagnostics)
        self.assertNotIn("test line 0\n", diagnostics)
        self.assertIn("dbus diagnostic", diagnostics)
        self.assertLessEqual(diagnostics.count("test line"), 80)
        self.assertLessEqual(len(diagnostics), 24_300)

    def test_requires_open_save_requests_to_reach_frontend_and_gtk_backend(self) -> None:
        log = "\n".join(
            [
                "interface=org.freedesktop.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.portal.FileChooser; member=SaveFile",
                "interface=org.freedesktop.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.impl.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.impl.portal.FileChooser; member=SaveFile",
                "interface=org.freedesktop.impl.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.portal.Request; member=Response",
                "interface=org.freedesktop.portal.Request; member=Response",
                "interface=org.freedesktop.portal.Request; member=Response",
            ]
        )
        require_portal_calls(log)

    def test_rejects_success_through_frontend_without_gtk_backend_calls(self) -> None:
        log = "\n".join(
            [
                "interface=org.freedesktop.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.portal.FileChooser; member=SaveFile",
                "interface=org.freedesktop.portal.FileChooser; member=OpenFile",
                "interface=org.freedesktop.portal.Request; member=Response",
                "interface=org.freedesktop.portal.Request; member=Response",
                "interface=org.freedesktop.portal.Request; member=Response",
            ]
        )
        with self.assertRaisesRegex(AssertionError, "GTK portal backend"):
            require_portal_calls(log)


class OwnedProcessCleanupTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform.startswith("linux"), "process-group cleanup is Linux-specific")
    def test_stops_and_reaps_the_owned_probe_process_group(self) -> None:
        child = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            start_new_session=True,
        )
        try:
            stop_owned_process(child)
            self.assertIsNotNone(child.poll())
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=3)

    def test_failure_stops_monitor_and_probe_and_removes_staging_directory(self) -> None:
        if not sys.platform.startswith("linux"):
            self.skipTest("the supervised portal process group is Linux-specific")

        real_popen = subprocess.Popen
        processes: list[subprocess.Popen[str]] = []
        tempdirs: list[Path] = []
        real_temp_directory = tempfile.TemporaryDirectory

        def make_sleeping_process(*_args: object, **kwargs: object) -> subprocess.Popen[str]:
            process = real_popen(
                [sys.executable, "-c", "import time; time.sleep(30)"],
                start_new_session=kwargs.get("start_new_session", False),
                stdout=kwargs.get("stdout"),
                stderr=kwargs.get("stderr"),
                text=kwargs.get("text", False),
            )
            processes.append(process)
            return process

        def record_temp_directory(*args: object, **kwargs: object) -> tempfile.TemporaryDirectory[str]:
            directory = real_temp_directory(*args, **kwargs)
            tempdirs.append(Path(directory.name))
            return directory

        with tempfile.TemporaryDirectory() as parent:
            cargo_json = Path(parent) / "cargo.jsonl"
            cargo_json.write_text("{}\n", encoding="utf-8")
            with (
                mock.patch.object(runner.shutil, "which", return_value="/usr/bin/fake"),
                mock.patch.object(runner.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)),
                mock.patch.object(runner.subprocess, "Popen", side_effect=make_sleeping_process),
                mock.patch.object(runner, "wait_for_monitor_ready"),
                mock.patch.object(runner, "wait_for_stage", side_effect=TimeoutError("injected stage timeout")),
                mock.patch.object(runner.tempfile, "TemporaryDirectory", side_effect=record_temp_directory),
                mock.patch.object(sys, "platform", "linux"),
            ):
                with self.assertRaisesRegex(TimeoutError, "injected stage timeout"):
                    runner.run_smoke(Path("/tmp/fake-portal-test"))

        self.assertEqual(len(processes), 2, "both the bus monitor and Rust probe should be owned")
        self.assertTrue(all(process.poll() is not None for process in processes), "all owned processes must be reaped")
        self.assertEqual(len(tempdirs), 1)
        self.assertFalse(tempdirs[0].exists(), "staging files must be removed on failure")

    def test_main_removes_cargo_json_when_selection_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            cargo_json = Path(temporary) / "cargo.jsonl"
            cargo_json.write_text("not json\n", encoding="utf-8")
            stderr = io.StringIO()
            with (
                mock.patch.object(sys, "argv", ["runner", "--cargo-json", str(cargo_json)]),
                contextlib.redirect_stderr(stderr),
            ):
                self.assertEqual(runner.main(), 1)
            self.assertFalse(cargo_json.exists(), "Cargo JSON staging file must be removed on failure")


if __name__ == "__main__":
    unittest.main()
