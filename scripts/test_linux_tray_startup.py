import importlib.util
import argparse
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).with_name("test-linux-tray-startup.py")
spec = importlib.util.spec_from_file_location("tray_startup_probe", SCRIPT)
probe = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = probe
spec.loader.exec_module(probe)
RUNNER = pathlib.Path(__file__).with_name("run-linux-tray-probe.py")
runner_spec = importlib.util.spec_from_file_location("tray_probe_runner", RUNNER)
runner = importlib.util.module_from_spec(runner_spec)
runner_spec.loader.exec_module(runner)


class ProbeCleanupTests(unittest.TestCase):
    def test_ephemeral_apparmor_profile_unloads_and_removes_copy_when_probe_fails(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-profile-cleanup-test-") as root:
            root_path = pathlib.Path(root)
            runner_temp = root_path / "runner-temp"
            runner_temp.mkdir()
            fake_bwrap = root_path / "bwrap-source"
            fake_bwrap.write_bytes(b"fake bwrap")
            commands = []

            def record_command(argv, **kwargs):
                commands.append(argv)
                return subprocess.CompletedProcess(argv, 0)

            args = argparse.Namespace(probe="probe.py", broker="broker", report="report.json")
            with mock.patch.dict(os.environ, {"RUNNER_TEMP": str(runner_temp)}), \
                 mock.patch.object(runner.shutil, "which", side_effect=lambda name: str(fake_bwrap) if name == "bwrap" else "/usr/bin/apparmor_parser"), \
                 mock.patch.object(runner, "needs_userns_profile", return_value=True), \
                 mock.patch.object(runner, "emit_runner_facts"), \
                 mock.patch.object(runner.shutil, "copy2", side_effect=lambda _src, dst: pathlib.Path(dst).write_bytes(b"private copy")), \
                 mock.patch.object(runner.subprocess, "run", side_effect=record_command), \
                 mock.patch.object(runner, "run_owned_probe", return_value=17) as probe_call:
                self.assertEqual(runner.run_probe(args), 17)

            self.assertEqual(len(commands), 2)
            self.assertEqual(commands[0][1], "-n")
            self.assertEqual(commands[1][1], "-n")
            self.assertEqual(commands[0][3], "-r")
            self.assertEqual(commands[1][3], "-R")
            self.assertEqual(commands[0][4], commands[1][4])
            self.assertFalse(pathlib.Path(commands[0][4]).exists())
            self.assertEqual(list(runner_temp.iterdir()), [])
            self.assertEqual(probe_call.call_count, 1)
            self.assertIn("QC_TRAY_PROBE_BWRAP", probe_call.call_args.args[1])

    def test_interruption_is_forwarded_only_to_owned_probe_and_returns_signal_status(self):
        handlers = {}

        class FakeProbe:
            pid = 4123
            sent = []
            complete = False
            def poll(self): return -2 if self.complete else None
            def send_signal(self, signum): self.sent.append(signum)
            def wait(self, *args, **kwargs):
                handlers[runner.signal.SIGTERM](runner.signal.SIGTERM, None)
                self.complete = True
                return -2

        child = FakeProbe()
        def fake_signal(signum, handler):
            previous = handlers.get(signum, runner.signal.SIG_DFL)
            handlers[signum] = handler
            return previous

        with mock.patch.object(runner.subprocess, "Popen", return_value=child), \
             mock.patch.object(runner, "owned_start_ticks", return_value="owned-start"), \
             mock.patch.object(runner.signal, "signal", side_effect=fake_signal):
            self.assertEqual(runner.run_owned_probe(["probe"], {}), 143)
        self.assertEqual(child.sent, [runner.signal.SIGINT])

    def test_profile_unloads_and_staging_disappears_when_interrupted_during_profile_load(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-profile-interrupt-test-") as root:
            root_path = pathlib.Path(root)
            runner_temp = root_path / "runner-temp"
            runner_temp.mkdir()
            fake_bwrap = root_path / "bwrap-source"
            fake_bwrap.write_bytes(b"fake bwrap")
            commands = []
            handlers = {}

            def record_command(argv, **kwargs):
                commands.append(argv)
                if argv[3] == "-r":
                    handlers[runner.signal.SIGTERM](runner.signal.SIGTERM, None)
                return subprocess.CompletedProcess(argv, 0)

            def fake_signal(signum, handler):
                previous = handlers.get(signum, runner.signal.SIG_DFL)
                handlers[signum] = handler
                return previous

            args = argparse.Namespace(probe="probe.py", broker="broker", report="report.json")
            with mock.patch.dict(os.environ, {"RUNNER_TEMP": str(runner_temp)}), \
                 mock.patch.object(runner.shutil, "which", side_effect=lambda name: str(fake_bwrap) if name == "bwrap" else "/usr/bin/apparmor_parser"), \
                 mock.patch.object(runner, "needs_userns_profile", return_value=True), \
                 mock.patch.object(runner, "emit_runner_facts"), \
                 mock.patch.object(runner.shutil, "copy2", side_effect=lambda _src, dst: pathlib.Path(dst).write_bytes(b"private copy")), \
                 mock.patch.object(runner.subprocess, "run", side_effect=record_command), \
                 mock.patch.object(runner.signal, "signal", side_effect=fake_signal), \
                 mock.patch.object(runner, "run_owned_probe") as probe_call:
                with self.assertRaises(runner.ProbeInterrupted):
                    runner.run_probe(args)
            probe_call.assert_not_called()
            self.assertEqual([command[3] for command in commands], ["-r", "-R"])
            self.assertFalse(pathlib.Path(commands[0][4]).exists())
            self.assertEqual(list(runner_temp.iterdir()), [])

    def test_bounded_diagnostic_tail_keeps_only_recent_lines_and_bytes(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-log-tail-test-") as root:
            log = pathlib.Path(root) / "sandbox.log"
            log.write_text("old\n" + "x" * 9000 + "\nlast\n", encoding="utf-8")
            tail = probe.bounded_log_tail(log, limit=32, lines=2)
            self.assertEqual(tail, "x" * 27 + "\nlast")

    def test_descriptor_claim_and_known_publication_sidecars_are_not_vacuously_ignored(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-known-residue-test-") as root:
            state = pathlib.Path(root)
            self.assertTrue(probe.claims_absent(state))
            for name in ("device-broker.json.claim", "device-broker.election", "device-broker.election.claim",
                         "device-broker.json.42.abc.tmp", "device-broker.election.42.abc.tmp"):
                path = state / name
                path.touch()
                self.assertFalse(probe.claims_absent(state), name)
                path.unlink()

    def test_identity_capture_failure_does_not_signal_an_unverified_process_group(self):
        real_popen = subprocess.Popen
        read_identity = probe.proc_identity
        launched = []
        def capture_owned_process(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            launched.append((child, read_identity(child.pid)))
            return child
        with tempfile.TemporaryDirectory(prefix="qc-tray-identity-failure-test-") as root:
            log_path = pathlib.Path(root) / "child.log"
            child = identity = None
            try:
                with mock.patch.object(probe.subprocess, "Popen", side_effect=capture_owned_process), \
                     mock.patch.object(probe, "proc_identity", return_value=None):
                    with self.assertRaises(probe.ProbeFailure) as raised:
                        probe.start_child([sys.executable, "-c", "import time; time.sleep(60)"],
                                          {}, log_path, role="broker")
                self.assertFalse(raised.exception.cleanup_verified)
                child, identity = launched[0]
                self.assertIsNotNone(identity)
                self.assertIsNone(child.poll(), "unverified child remains for enclosing namespace teardown")
                self.assertTrue(probe.group_has_live_members(child.pid))
            finally:
                if child is None and launched:
                    child, identity = launched[0]
                if child is not None and identity is not None and child.poll() is None:
                    self.assertTrue(probe.stop_owned_group(child, identity, grace=1))

    def test_failure_case_reaps_only_its_child_group_and_fixture_state(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-probe-cleanup-test-") as root:
            base = pathlib.Path(root)
            state = base / "state-injected-failure"
            launched = []

            def harmless_owned_child(_binary, child_state, _env, log_path):
                child_state.mkdir(mode=0o700, parents=True)
                child = subprocess.Popen(
                    [sys.executable, "-c",
                    "import pathlib,signal,sys,time; p=pathlib.Path(sys.argv[1]); "
                     "signal.signal(signal.SIGTERM,lambda *_:(p.unlink(missing_ok=True),sys.exit(0))); "
                     "pathlib.Path(sys.argv[2]).touch(); time.sleep(60)",
                     str(child_state / "device-broker.json"), str(child_state / "child-ready")],
                    stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    start_new_session=True,
                )
                identity = probe.proc_identity(child.pid)
                launched.append((child, identity))
                ready_deadline = probe.time.monotonic() + 3
                while not (child_state / "child-ready").exists() and probe.time.monotonic() < ready_deadline:
                    probe.time.sleep(0.01)
                if not (child_state / "child-ready").exists(): raise RuntimeError("fixture child did not initialize")
                (child_state / "child-ready").unlink()
                (child_state / "device-broker.json").write_text(
                    '{"processId":%d}\n' % child.pid, encoding="utf-8")
                return child, identity

            isolation = {"privateDev": True, "privateSys": True, "privatePidNamespace": True,
                         "privateNetworkNamespace": True, "usbOrHidNodesVisible": False}
            try:
                with mock.patch.object(probe, "disconnected_checks", return_value=isolation), \
                     mock.patch.object(probe, "launch_broker_case", side_effect=harmless_owned_child):
                    result = probe.run_case("injected-invalid-scenario", "/fixture/qc-device-broker", base, {})

                self.assertEqual(result["failureCode"], "UnknownScenario")
                self.assertTrue(result["ownedBrokerGroupReaped"])
                self.assertTrue(result["claimsRemoved"], str(result))
                self.assertTrue(result["fixtureStateRemoved"])
                self.assertTrue(result["cleanupVerified"])
                self.assertFalse(result["passed"])
                child, identity = launched[0]
                self.assertIsNotNone(identity)
                self.assertIsNotNone(child.poll())
                self.assertFalse(probe.group_has_live_members(child.pid))
                self.assertFalse(state.exists())
            finally:
                for child, identity in launched:
                    if identity is not None and child.poll() is None:
                        self.assertTrue(probe.stop_owned_group(child, identity, grace=1))

    def test_normal_exit_wait_is_bounded_and_preserves_staging_for_a_live_group(self):
        clock = [0.0]
        def advance(seconds): clock[0] += seconds
        with mock.patch.object(probe, "group_has_live_members", return_value=True), \
             mock.patch.object(probe.time, "monotonic", side_effect=lambda: clock[0]), \
             mock.patch.object(probe.time, "sleep", side_effect=advance), \
             mock.patch.object(probe.os, "killpg") as signal_group:
            clean = probe.wait_for_group_exit(7000, grace=0.15)
        self.assertFalse(clean)
        self.assertAlmostEqual(clock[0], 0.15)
        signal_group.assert_not_called()
        with tempfile.TemporaryDirectory(prefix="qc-tray-group-retained-") as root:
            staging = pathlib.Path(tempfile.mkdtemp(prefix="qc-tray-startup-test-", dir=root))
            report = {"sandboxGroupReaped": clean, "innerCleanupVerified": True, "ok": True}
            probe.finalize_outer_report(pathlib.Path(root) / "report.json", report, staging, not clean, "test")
            self.assertFalse(report["ok"])
            self.assertFalse(report["cleanupVerified"])
            self.assertTrue(staging.exists())
            self.assertEqual(report["stagingPathPreserved"], str(staging))

    def test_group_survivor_diagnostics_are_bounded_without_reading_command_lines(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-group-facts-") as root:
            entries = []
            for pid in [10, 11]:
                entry = pathlib.Path(root) / str(pid)
                entry.mkdir()
                fields = ["S", "1", "7000"] + ["0"] * 17
                fields[19] = "1234"
                (entry / "stat").write_text(f"{pid} (owned child) " + " ".join(fields))
                entries.append(entry)
            with mock.patch.object(probe.pathlib.Path, "iterdir", return_value=iter(entries)), \
                 mock.patch.object(probe.os, "readlink", return_value="/fixture/owned"), \
                 mock.patch.object(probe, "proc_identity") as unrelated_identity:
                facts = probe.group_survivor_diagnostics(7000, limit=1)
            self.assertEqual(facts, {"members": [{"pid": 10, "startTicks": 1234, "ppid": 1,
                                                   "pgrp": 7000, "exe": "/fixture/owned"}], "truncated": True})
            unrelated_identity.assert_not_called()

    @unittest.skipUnless(sys.platform.startswith("linux"), "requires Linux procfs process groups")
    def test_normal_launcher_exit_waits_for_descendant_and_leaves_sibling_alive(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-group-settle-") as root:
            release = pathlib.Path(root) / "release"
            child_script = ("import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\ndeadline=time.monotonic()+10\n"
                            "while not p.exists() and time.monotonic()<deadline: time.sleep(0.01)\n"
                            "if p.exists(): time.sleep(0.15)\n")
            parent_script = ("import subprocess,sys; p=subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]]); "
                             "print(p.pid,flush=True)")
            parent = sibling = None
            try:
                parent = subprocess.Popen([sys.executable, "-c", parent_script, child_script, str(release)],
                                          stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, start_new_session=True)
                sibling = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
                self.assertTrue(parent.stdout.readline().strip().isdigit())
                self.assertEqual(parent.wait(timeout=3), 0)
                self.assertTrue(probe.group_has_live_members(parent.pid), "the child outlives its launcher")
                release.touch()
                self.assertTrue(probe.wait_for_group_exit(parent.pid, grace=3))
                self.assertIsNone(sibling.poll(), "unrelated processes must survive group retirement")
            finally:
                release.touch()
                try:
                    if parent is not None:
                        try: parent.wait(timeout=3)
                        except subprocess.TimeoutExpired:
                            parent.terminate()
                            parent.wait(timeout=3)
                        self.assertTrue(probe.wait_for_group_exit(parent.pid, grace=12))
                finally:
                    if parent is not None and parent.stdout is not None: parent.stdout.close()
                    if sibling is not None:
                        sibling.terminate()
                        sibling.wait(timeout=3)

    def test_report_does_not_claim_staging_cleanup_before_rmtree_succeeds(self):
        real_rmtree = shutil.rmtree
        with tempfile.TemporaryDirectory(prefix="qc-tray-report-cleanup-test-") as root:
            base = pathlib.Path(root)
            staging = pathlib.Path(tempfile.mkdtemp(prefix="qc-tray-startup-test-", dir=base))
            report_path = base / "report.json"
            report = {"sandboxGroupReaped": True, "innerCleanupVerified": True, "ok": True}
            with mock.patch.object(probe.shutil, "rmtree", side_effect=PermissionError("fixture")):
                probe.finalize_outer_report(report_path, report, staging, False, "test")
            import json
            saved = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertFalse(saved["stagingRemoved"])
            self.assertFalse(saved["cleanupVerified"])
            self.assertFalse(saved["ok"])
            self.assertEqual(saved["stagingPathPreserved"], str(staging))
            self.assertTrue(staging.is_dir())
            real_rmtree(staging)

    def test_successful_finalization_sets_cleanup_only_after_tree_is_gone(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-report-success-test-") as root:
            base = pathlib.Path(root)
            staging = pathlib.Path(tempfile.mkdtemp(prefix="qc-tray-startup-test-", dir=base))
            report_path = base / "report.json"
            report = {"sandboxGroupReaped": True, "innerCleanupVerified": True, "ok": True}
            probe.finalize_outer_report(report_path, report, staging, False, "test")
            import json
            saved = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertFalse(staging.exists())
            self.assertTrue(saved["stagingRemoved"])
            self.assertTrue(saved["cleanupVerified"])
            self.assertTrue(saved["ok"])

    def test_reaped_sandbox_before_inner_start_cleans_staging_without_claiming_probe_pass(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-preinner-cleanup-test-") as root:
            base = pathlib.Path(root)
            staging = pathlib.Path(tempfile.mkdtemp(prefix="qc-tray-startup-test-", dir=base))
            report_path = base / "report.json"
            report = {"sandboxGroupReaped": True, "innerStarted": False, "ok": False,
                      "failureStage": "sandbox_setup", "failureCode": "BubblewrapSetupFailed"}
            probe.finalize_outer_report(report_path, report, staging, False, "test")
            import json
            saved = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertFalse(staging.exists())
            self.assertTrue(saved["stagingRemoved"])
            self.assertTrue(saved["cleanupVerified"])
            self.assertFalse(saved["ok"])
            self.assertNotIn("innerCleanupVerified", saved)

    @unittest.skipUnless(__import__("os").environ.get("DBUS_SESSION_BUS_ADDRESS"),
                         "run under dbus-run-session to exercise the real Gio fixture")
    def test_gio_query_reads_false_host_property_from_live_synthetic_watcher(self):
        with tempfile.TemporaryDirectory(prefix="qc-tray-watcher-query-test-") as root:
            base = pathlib.Path(root)
            ready, registered = base / "ready.json", base / "registered.json"
            watcher, identity = probe.start_child(
                [probe.SYSTEM_PYTHON, str(SCRIPT), "--watcher", str(ready), str(registered), "false"],
                __import__("os").environ.copy(), base / "watcher.log", role="watcher")
            try:
                self.assertTrue(probe.wait_file(ready, watcher, __import__("time").monotonic() + 5))
                self.assertIs(probe.query_watcher_host_registered(), False)
            finally:
                self.assertTrue(probe.stop_owned_group(watcher, identity, grace=1))


if __name__ == "__main__":
    unittest.main()
