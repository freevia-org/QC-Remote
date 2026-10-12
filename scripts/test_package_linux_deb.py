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


def _fixture_deb(
    path: Path,
    files: dict[str, bytes],
    *,
    data_member: str = "data.tar.xz",
    data_payload: bytes | None = None,
) -> None:
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
    payload = data_tar.getvalue() if data_payload is None else data_payload
    path.write_bytes(
        b"!<arch>\n"
        + _ar_member("debian-binary", b"2.0\n")
        + _ar_member("control.tar.xz", empty_tar.getvalue())
        + _ar_member(data_member, payload)
    )


@unittest.skipUnless(sys.platform == "linux", "process-group cleanup is Linux-only")
class PackageSafetyTests(unittest.TestCase):
    def test_expected_deb_app_hash_uses_private_copy_and_cleans_it(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-patch-") as temporary:
            root = Path(temporary)
            source = root / "app"
            destination = root / "tmp/qc-remote-expected-deb"
            destination.parent.mkdir()
            token = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
            replacement = b"__TAURI_BUNDLE_TYPE_VAR_DEB"
            prefix = b"x" * (1024 * 1024 - 9)
            original = prefix + token + b"tail"
            source.write_bytes(original)
            source.chmod(0o700)

            result = package_linux_deb.expected_deb_app_sha256(source, destination)

            self.assertEqual(result, hashlib.sha256(prefix + replacement + b"tail").hexdigest())
            self.assertEqual(source.read_bytes(), original)
            self.assertFalse(destination.exists(), "private patched app copy survived cleanup")

    def test_expected_deb_app_hash_rejects_absent_or_duplicate_marker_and_cleans_copy(self) -> None:
        token = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
        cases = ((b"no marker", 0), (token + b"and" + token, 2))
        for payload, count in cases:
            with self.subTest(marker_count=count), tempfile.TemporaryDirectory(prefix="qc-deb-patch-invalid-") as temporary:
                root = Path(temporary)
                source = root / "app"
                destination = root / "tmp/qc-remote-expected-deb"
                destination.parent.mkdir()
                source.write_bytes(payload)
                source.chmod(0o700)

                with self.assertRaisesRegex(package_linux_deb.PackageError, f"found {count}"):
                    package_linux_deb.expected_deb_app_sha256(source, destination)

                self.assertFalse(destination.exists(), "rejected private app copy survived cleanup")

    def test_binary_command_output_collision_preserves_preexisting_file(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-binary-collision-") as temporary:
            root = Path(temporary)
            output = root / "data.tar"
            output.write_bytes(b"belongs to another operation")
            with self.assertRaises(FileExistsError):
                package_linux_deb.run_owned_to_file(
                    [sys.executable, "-c", "raise SystemExit(0)"],
                    cwd=root,
                    timeout=2,
                    output=output,
                )
            self.assertEqual(output.read_bytes(), b"belongs to another operation")

    def test_timed_out_binary_command_reaps_descendant_and_removes_partial_tar(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-binary-process-") as temporary:
            root = Path(temporary)
            pid_file = root / "child.pid"
            output = root / "data.tar"
            code = (
                "import os,pathlib,sys,time; "
                "pid=os.fork(); "
                "(pathlib.Path(os.environ['QC_CHILD_PID']).write_text(str(os.getpid())),time.sleep(60)) "
                "if pid==0 else (sys.stdout.buffer.write(b'partial'),sys.stdout.flush(),sys.exit(7))"
            )
            env = os.environ.copy()
            env["QC_CHILD_PID"] = str(pid_file)
            with self.assertRaisesRegex(package_linux_deb.PackageError, "timed out"):
                package_linux_deb.run_owned_to_file(
                    [sys.executable, "-c", code], cwd=root, timeout=0.5, output=output, env=env,
                )

            self.assertFalse(output.exists(), "partial tar survived child timeout cleanup")
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
                self.fail(f"owned binary-output child {child_pid} survived process-group cleanup")

    def test_zstd_data_member_uses_dpkg_normalization_and_hash_checks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-zstd-") as temporary:
            root = Path(temporary)
            deb = root / "candidate.deb"
            app = b"QC Remote zstd fixture"
            broker = b"QC broker zstd fixture"
            files = {
                "/usr/bin/qc-remote": app,
                "/usr/bin/qc-device-broker": broker,
                package_linux_deb.PACKAGE_RULE_PATH: b"scoped rules",
            }
            data_tar = io.BytesIO()
            with tarfile.open(fileobj=data_tar, mode="w") as archive:
                for member_path, contents in files.items():
                    info = tarfile.TarInfo("./" + member_path.lstrip("/"))
                    info.mode = 0o755 if member_path.endswith(("qc-remote", "qc-device-broker")) else 0o644
                    info.size = len(contents)
                    archive.addfile(info, io.BytesIO(contents))
            _fixture_deb(deb, files, data_member="data.tar.zst", data_payload=b"fixture-zstd-bytes")

            captured: list[Path] = []

            def normalize(_command, *, cwd, timeout, output) -> None:
                self.assertEqual(timeout, 120)
                self.assertEqual(cwd, root)
                captured.append(output)
                output.write_bytes(data_tar.getvalue())

            with patch.object(package_linux_deb, "run_owned_to_file", side_effect=normalize) as normalizer:
                package_linux_deb.verify_deb(
                    deb,
                    target="x86_64-unknown-linux-gnu",
                    expected_hashes={
                        "/usr/bin/qc-remote": hashlib.sha256(app).hexdigest(),
                        "/usr/bin/qc-device-broker": hashlib.sha256(broker).hexdigest(),
                        package_linux_deb.PACKAGE_RULE_PATH: hashlib.sha256(b"scoped rules").hexdigest(),
                    },
                )

            normalizer.assert_called_once()
            self.assertEqual(len(captured), 1)
            self.assertFalse(captured[0].exists(), "normalized temporary tar survived verification")

    def test_failed_zstd_normalization_removes_partial_tar(self) -> None:
        with tempfile.TemporaryDirectory(prefix="qc-deb-zstd-failure-") as temporary:
            root = Path(temporary)
            deb = root / "candidate.deb"
            _fixture_deb(deb, {}, data_member="data.tar.zst", data_payload=b"fixture-zstd-bytes")
            captured: list[Path] = []

            def fail_after_partial_write(_command, *, cwd, timeout, output) -> None:
                captured.append(output)
                output.write_bytes(b"partial tar")
                raise package_linux_deb.PackageError("simulated dpkg-deb failure")

            with patch.object(package_linux_deb, "run_owned_to_file", side_effect=fail_after_partial_write):
                with self.assertRaisesRegex(package_linux_deb.PackageError, "simulated dpkg-deb failure"):
                    with package_linux_deb.open_deb_data_tar(deb):
                        self.fail("failed normalization unexpectedly yielded a tar")

            self.assertEqual(len(captured), 1)
            self.assertFalse(captured[0].exists(), "partial normalized tar survived failure cleanup")

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

    def test_full_package_verifies_the_exact_staged_broker_after_tauri_rebuilds_target(self) -> None:
        for tamper_staged_package in (False, True):
            with self.subTest(tamper_staged_package=tamper_staged_package), tempfile.TemporaryDirectory(
                prefix="qc-deb-rebuilt-sidecar-"
            ) as temporary:
                root = Path(temporary)
                app = root / "app"
                target_dir = root / "target"
                rules = root / "70-qc-remote.rules"
                app.mkdir()
                rules.write_bytes(b"scoped udev rules")
                target = "x86_64-unknown-linux-gnu"
                broker_path = target_dir / target / "release/qc-device-broker"
                app_path = target_dir / target / "release/qc-remote"
                original_broker = b"the verified broker that was staged"
                tauri_rebuilt_broker = b"the broker Cargo rebuilt during Tauri bundling"
                bundle_marker = b"__TAURI_BUNDLE_TYPE_VAR_UNK"
                patched_app = b"app-prefix" + b"__TAURI_BUNDLE_TYPE_VAR_DEB" + b"app-suffix"

                def fake_run_owned(command, *, cwd, timeout, env=None):
                    if command[:2] == ["rustc", "-vV"]:
                        return "host: x86_64-unknown-linux-gnu\n"
                    if command[:3] == ["npm", "run", "build"]:
                        return ""
                    if command[:2] == ["cargo", "build"]:
                        broker_path.parent.mkdir(parents=True)
                        broker_path.write_bytes(original_broker)
                        broker_path.chmod(0o700)
                        app_path.write_bytes(b"app-prefix" + bundle_marker + b"app-suffix")
                        app_path.chmod(0o700)
                        return ""
                    if command[:3] == ["npm", "run", "tauri:build"]:
                        sidecar_path = app / "binaries" / f"qc-device-broker-{target}"
                        packaged_broker = sidecar_path.read_bytes()
                        if tamper_staged_package:
                            packaged_broker += b" altered"
                        # Tauri recompiles the Cargo-discovered binary after
                        # the external sidecar was staged. The Debian bundle
                        # must still contain the exact staged external binary.
                        broker_path.write_bytes(tauri_rebuilt_broker)
                        deb_dir = target_dir / target / "release/bundle/deb"
                        deb_dir.mkdir(parents=True)
                        _fixture_deb(
                            deb_dir / "candidate.deb",
                            {
                                "/usr/bin/qc-remote": patched_app,
                                "/usr/bin/qc-device-broker": packaged_broker,
                                package_linux_deb.PACKAGE_RULE_PATH: rules.read_bytes(),
                            },
                        )
                        return ""
                    if len(command) == 2 and command[1] == "--self-check":
                        return ""
                    raise AssertionError(f"unexpected package command: {command!r}")

                with (
                    patch.object(package_linux_deb, "APP", app),
                    patch.object(package_linux_deb, "RULES", rules),
                    patch.object(package_linux_deb, "run_owned", side_effect=fake_run_owned),
                    patch.dict(
                        os.environ,
                        {"CARGO_TARGET_DIR": str(target_dir), "RUSTUP_TOOLCHAIN": "stable"},
                    ),
                ):
                    if tamper_staged_package:
                        with self.assertRaisesRegex(package_linux_deb.PackageError, "unexpected binary bytes"):
                            package_linux_deb.package(timeout=30)
                    else:
                        result = package_linux_deb.package(timeout=30)
                        self.assertTrue(result.is_file())
                    self.assertFalse(
                        (app / "binaries" / f"qc-device-broker-{target}").exists(),
                        "temporary staged sidecar survived package verification",
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
