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
import time
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
    def test_dbus_activation_receives_xvfb_display_authority(self) -> None:
        self.assertIn("DISPLAY", runner.DBUS_ACTIVATION_ENVIRONMENT)
        self.assertIn("XAUTHORITY", runner.DBUS_ACTIVATION_ENVIRONMENT)

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

    @unittest.skipUnless(sys.platform.startswith("linux"), "process-group cleanup is Linux-specific")
    def test_kills_descendant_when_group_leader_exits_before_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            child_pid_file = Path(temporary) / "child.pid"
            child_ready_file = Path(temporary) / "child.ready"
            leader = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    (
                        "import pathlib,signal,subprocess,sys,time; "
                        "child=subprocess.Popen([sys.executable,'-c',"
                        "'import pathlib,signal,sys,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); "
                        "pathlib.Path(sys.argv[1]).write_text(\"ready\"); time.sleep(60)',sys.argv[2]]); "
                        "pathlib.Path(sys.argv[1]).write_text(str(child.pid)); "
                        "signal.signal(signal.SIGTERM, signal.SIG_DFL); time.sleep(60)"
                    ),
                    str(child_pid_file),
                    str(child_ready_file),
                ],
                start_new_session=True,
            )
            child_pid: int | None = None
            try:
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and not (
                    child_ready_file.exists() and child_pid_file.exists()
                ):
                    time.sleep(0.01)
                self.assertTrue(child_ready_file.exists(), "child did not install its signal handler")
                self.assertTrue(child_pid_file.exists(), "leader did not record its child")
                child_pid = int(child_pid_file.read_text(encoding="utf-8"))

                # Let the leader exit first while its child remains in the
                # same process group and ignores the graceful signal.
                os.kill(leader.pid, signal.SIGTERM)
                leader.wait(timeout=3)
                self.assertTrue(runner.process_group_has_live_members(leader.pid))

                stop_owned_process(leader)

                self.assertFalse(
                    runner.process_group_has_live_members(leader.pid),
                    "owned process group must have no live process before unmount or staging removal",
                )
                child_stat = Path(f"/proc/{child_pid}/stat")
                if child_stat.exists():
                    child_fields = child_stat.read_text(encoding="ascii").rsplit(")", 1)[1].split()
                    self.assertIn(child_fields[0], {"Z", "X"}, "descendant must be exited")
            finally:
                if runner.process_group_has_live_members(leader.pid):
                    os.killpg(leader.pid, signal.SIGKILL)
                    runner.wait_for_process_group_exit(leader.pid, timeout=3)
                if leader.poll() is None:
                    leader.kill()
                leader.wait(timeout=3)

    def test_failure_stops_and_reaps_owned_monitor_and_probe(self) -> None:
        if not sys.platform.startswith("linux"):
            self.skipTest("the supervised portal process group is Linux-specific")

        class FakeProcess:
            def __init__(self) -> None:
                self.pid = 12345
                self.returncode: int | None = None

            def poll(self) -> int | None:
                return self.returncode

            def wait(self, timeout: float | None = None) -> int:
                self.returncode = -signal.SIGTERM
                return self.returncode

            def send_signal(self, _signal: int) -> None:
                self.returncode = -signal.SIGINT

            def kill(self) -> None:
                self.returncode = -signal.SIGKILL

        processes: list[FakeProcess] = []

        def make_process(*_args: object, **_kwargs: object) -> FakeProcess:
            process = FakeProcess()
            processes.append(process)
            return process

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runner.prepare_staging(root)
            with (
                mock.patch.object(runner.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)),
                mock.patch.object(runner.subprocess, "Popen", side_effect=make_process),
                mock.patch.object(runner, "wait_for_monitor_ready"),
                mock.patch.object(runner, "wait_for_stage", side_effect=TimeoutError("injected stage timeout")),
                mock.patch.object(runner.os, "killpg"),
                mock.patch.object(runner, "process_group_has_live_members", return_value=False),
                mock.patch.dict(os.environ, {"DISPLAY": ":99", "XAUTHORITY": "/tmp/xauth"}),
            ):
                with self.assertRaisesRegex(TimeoutError, "injected stage timeout"):
                    runner.run_session_worker(Path("/tmp/fake-portal-test"), root)

        self.assertEqual(len(processes), 2, "both the bus monitor and Rust probe should be owned")
        self.assertTrue(all(process.poll() is not None for process in processes), "all owned processes must be reaped")

    def test_parent_keeps_staging_until_dbus_session_exits(self) -> None:
        tempdirs: list[Path] = []
        real_temp_directory = tempfile.TemporaryDirectory

        def record_temp_directory(*args: object, **kwargs: object) -> tempfile.TemporaryDirectory[str]:
            directory = real_temp_directory(*args, **kwargs)
            tempdirs.append(Path(directory.name))
            return directory

        class FakeSession:
            pid = 23456
            returncode = 0

            def poll(self) -> int:
                return 0

            def wait(self, timeout: float | None = None) -> int:
                root = tempdirs[0]
                self_outer.assertTrue(root.exists())
                doc_mount = root / "runtime" / "doc"
                doc_mount.mkdir()
                (root / "saved-by-portal.txt").write_text("qc-remote-portal-save-result\n", encoding="utf-8")
                return 0

        self_outer = self
        cleanup_order: list[str] = []
        original_unmount = runner.unmount_document_portal

        def stop_session(session: object) -> None:
            cleanup_order.append("stop-session")

        def unmount(root: Path) -> None:
            cleanup_order.append("unmount")
            original_unmount(root)
        with (
            mock.patch.object(runner.shutil, "which", return_value="/usr/bin/fake"),
            mock.patch.object(runner.subprocess, "Popen", return_value=FakeSession()),
            mock.patch.object(
                runner.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(["fusermount3", "-u"], 0, "", ""),
            ) as run,
            mock.patch.object(runner.tempfile, "TemporaryDirectory", side_effect=record_temp_directory),
            mock.patch.object(runner, "document_portal_mount_is_present", side_effect=[True, False]),
            mock.patch.object(runner, "stop_owned_process", side_effect=stop_session),
            mock.patch.object(runner, "unmount_document_portal", side_effect=unmount),
            mock.patch.object(sys, "platform", "linux"),
        ):
            runner.run_smoke(Path("/tmp/fake-portal-test"))

        self.assertEqual(len(tempdirs), 1)
        self.assertEqual(cleanup_order, ["stop-session", "unmount"])
        run.assert_called_once()
        self.assertEqual(run.call_args.args[0][1], "-u")
        self.assertTrue(str(tempdirs[0] / "runtime" / "doc") in run.call_args.args[0])
        self.assertFalse(tempdirs[0].exists(), "parent staging is removed only after D-Bus session exit")

    def test_disconnected_document_portal_uses_verified_lazy_detach(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            mountpoint = Path(temporary) / "runtime" / "doc"
            mountpoint.mkdir(parents=True)
            disconnected = subprocess.CompletedProcess(
                ["fusermount3", "-u"], 1, "", "fusermount3: Transport endpoint is not connected"
            )
            detached = subprocess.CompletedProcess(["fusermount3", "-uz"], 0, "", "")
            with (
                mock.patch.object(runner.shutil, "which", return_value="/usr/bin/fusermount3"),
                mock.patch.object(runner.subprocess, "run", side_effect=[disconnected, detached]) as run,
                mock.patch.object(runner, "document_portal_mount_is_present", side_effect=[True, True, False]),
            ):
                runner.unmount_document_portal(Path(temporary))
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args_list[0].args[0][1], "-u")
            self.assertEqual(run.call_args_list[1].args[0][1], "-uz")

    def test_already_detached_document_portal_is_accepted_by_mount_table(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            mountpoint = Path(temporary) / "runtime" / "doc"
            mountpoint.mkdir(parents=True)
            disconnected = subprocess.CompletedProcess(
                ["fusermount3", "-u"], 1, "", "fusermount3: Transport endpoint is not connected"
            )
            with (
                mock.patch.object(runner.shutil, "which", return_value="/usr/bin/fusermount3"),
                mock.patch.object(runner.subprocess, "run", return_value=disconnected) as run,
                mock.patch.object(runner, "document_portal_mount_is_present", side_effect=[True, False]),
            ):
                runner.unmount_document_portal(Path(temporary))
            run.assert_called_once()

    def test_mountinfo_matches_exact_escaped_mountpoint(self) -> None:
        mountpoint = Path("/tmp/qc portal/runtime/doc")
        line = "42 23 0:42 / /tmp/qc\\040portal/runtime/doc rw,nosuid - fuse.portal portal rw"
        self.assertTrue(runner._mountinfo_has_mountpoint(line, mountpoint))
        self.assertFalse(runner._mountinfo_has_mountpoint(line, Path("/tmp/qc portal/runtime/doc-other")))

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
