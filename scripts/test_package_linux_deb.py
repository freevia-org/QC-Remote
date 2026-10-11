"""Focused safety tests for package-linux-deb.py; never invokes Cargo or npm."""

from __future__ import annotations

import importlib.util
import hashlib
import io
import json
from pathlib import Path
import os
import sys
import tempfile
import tarfile
import time
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("package-linux-deb.py")
SPEC = importlib.util.spec_from_file_location("package_linux_deb", SCRIPT)
assert SPEC and SPEC.loader
package_linux_deb = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = package_linux_deb
SPEC.loader.exec_module(package_linux_deb)


def _ar_member(name: str, payload: bytes) -> bytes:
    header = (
        f"{name + '/':<16}{0:<12}{0:<6}{0:<6}{0o100644:<8o}{len(payload):<10}`\n"
    ).encode("ascii")
    return header + payload + (b"\n" if len(payload) & 1 else b"")


def _fixture_deb(path: Path, files: dict[str, bytes]) -> None:
    data_tar = io.BytesIO()
    with tarfile.open(fileobj=data_tar, mode="w:xz") as archive:
        for member_path, contents in files.items():
            info = tarfile.TarInfo("./" + member_path.lstrip("/"))
            info.mode = 0o755 if member_path.endswith(("qc-remote", "qc-device-broker")) else 0o644
            info.size = len(contents)
            archive.addfile(info, io.BytesIO(contents))
    empty_tar = io.BytesIO()
    with tarfile.open(fileobj=empty_tar, mode="w:xz"):
        pass
    path.write_bytes(
        b"!<arch>\n"
        + _ar_member("debian-binary", b"2.0\n")
        + _ar_member("control.tar.xz", empty_tar.getvalue())
        + _ar_member("data.tar.xz", data_tar.getvalue())
    )


@unittest.skipUnless(sys.platform == "linux", "process-group cleanup is Linux-only")
class PackageSafetyTests(unittest.TestCase):
    def test_failed_package_publication_removes_only_its_partial_output(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-publish-") as temporary:
            root = Path(temporary)
            source = root / "candidate.deb"
            output = root / "published.deb"
            source.write_bytes(b"complete verified package")

            with patch.object(
                package_linux_deb.shutil,
                "copyfileobj",
                side_effect=OSError("simulated interrupted publication"),
            ):
                with self.assertRaisesRegex(OSError, "interrupted publication"):
                    package_linux_deb.publish_package(source, output)

            self.assertFalse(output.exists(), "partial output owned by this call was retained")
            self.assertEqual(source.read_bytes(), b"complete verified package")

    def test_linux_bundle_maps_the_scoped_rules_into_the_deb(self) -> None:
        config_path = package_linux_deb.APP / "tauri.linux.conf.json"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        files = config["bundle"]["linux"]["deb"]["files"]
        self.assertEqual(
            files[package_linux_deb.PACKAGE_RULE_PATH],
            "../../../services/device-broker/linux/70-qc-remote.rules",
        )
        self.assertIn("152a", package_linux_deb.RULES.read_text(encoding="utf-8"))

    def test_package_verification_requires_broker_app_and_rules(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-verify-") as temporary:
            deb = Path(temporary) / "candidate.deb"
            app = b"QC Remote fixture"
            broker = b"QC broker fixture"
            _fixture_deb(
                deb,
                {
                    "/usr/bin/qc-remote": app,
                    "/usr/bin/qc-device-broker": broker,
                    package_linux_deb.PACKAGE_RULE_PATH: b"scoped rules",
                },
            )
            package_linux_deb.verify_deb(
                deb,
                target="x86_64-unknown-linux-gnu",
                expected_hashes={
                    "/usr/bin/qc-remote": hashlib.sha256(app).hexdigest(),
                    "/usr/bin/qc-device-broker": hashlib.sha256(broker).hexdigest(),
                    package_linux_deb.PACKAGE_RULE_PATH: hashlib.sha256(b"scoped rules").hexdigest(),
                },
            )
            _fixture_deb(
                deb,
                {
                    "/usr/bin/qc-remote": app,
                    "/usr/lib/qc-remote/qc-device-broker": broker,
                    package_linux_deb.PACKAGE_RULE_PATH: b"scoped rules",
                },
            )
            with self.assertRaisesRegex(package_linux_deb.PackageError, "exact package members"):
                package_linux_deb.verify_deb(
                    deb,
                    target="x86_64-unknown-linux-gnu",
                    expected_hashes={"/usr/bin/qc-remote": hashlib.sha256(app).hexdigest()},
                )

    def test_staged_sidecar_is_removed_after_package_failure(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-stage-") as temporary:
            root = Path(temporary)
            source = root / "broker"
            source.write_bytes(b"fixture executable")
            source.chmod(0o700)
            destination = root / "binaries/qc-device-broker-x86_64-unknown-linux-gnu"

            with self.assertRaisesRegex(RuntimeError, "simulated package failure"):
                with package_linux_deb.stage_sidecar(source, destination):
                    self.assertEqual(destination.read_bytes(), b"fixture executable")
                    self.assertTrue(destination.stat().st_mode & 0o111)
                    raise RuntimeError("simulated package failure")

            self.assertFalse(destination.exists())
            self.assertFalse(destination.parent.exists())

    def test_staging_refuses_to_replace_an_existing_sidecar(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-collision-") as temporary:
            root = Path(temporary)
            source = root / "broker"
            source.write_bytes(b"new broker")
            source.chmod(0o700)
            destination = root / "binaries/qc-device-broker-x86_64-unknown-linux-gnu"
            destination.parent.mkdir()
            destination.write_bytes(b"owned by another invocation")

            with self.assertRaises(FileExistsError):
                with package_linux_deb.stage_sidecar(source, destination):
                    self.fail("exclusive staging unexpectedly replaced an existing path")
            self.assertEqual(destination.read_bytes(), b"owned by another invocation")

    def test_timed_out_command_reaps_its_descendant_group(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-process-") as temporary:
            pid_file = Path(temporary) / "child.pid"
            code = (
                "import os,time,pathlib; "
                "pid=os.fork(); "
                "(pathlib.Path(os.environ['QC_CHILD_PID']).write_text(str(os.getpid())), time.sleep(60)) "
                "if pid == 0 else time.sleep(60)"
            )
            env = os.environ.copy()
            env["QC_CHILD_PID"] = str(pid_file)
            with self.assertRaisesRegex(package_linux_deb.PackageError, "timed out"):
                package_linux_deb.run_owned(
                    [sys.executable, "-c", code], cwd=Path(temporary), timeout=0.5, env=env
                )

            self.assertTrue(pid_file.exists(), "fixture child did not start")
            child_pid = int(pid_file.read_text(encoding="ascii"))
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    stat = Path(f"/proc/{child_pid}/stat").read_text(encoding="ascii")
                    process_state = stat[stat.rfind(")") + 2]
                except FileNotFoundError:
                    break
                if process_state == "Z":
                    break
                time.sleep(0.025)
            else:
                self.fail(f"owned child process {child_pid} survived process-group cleanup")


if __name__ == "__main__":
    unittest.main()
