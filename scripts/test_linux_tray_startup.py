import importlib.util
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


class ProbeCleanupTests(unittest.TestCase):
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
