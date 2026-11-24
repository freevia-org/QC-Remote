#!/usr/bin/env python3
"""Drive the ignored rfd portal smoke test inside an isolated XDG session."""

from __future__ import annotations

import argparse
import json
import os
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

OPEN_TITLE = "QC Remote Portal Open Smoke"
SAVE_TITLE = "QC Remote Portal Save Smoke"
CANCEL_TITLE = "QC Remote Portal Cancel Smoke"
TEST_NAME = "xdg_portal_open_save_cancel_smoke"
MAX_TEST_SECONDS = 90
MAX_DIALOG_SECONDS = 20
MAX_SESSION_SECONDS = 150
DBUS_ACTIVATION_ENVIRONMENT = (
    "XDG_CONFIG_HOME",
    "XDG_CURRENT_DESKTOP",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "GDK_BACKEND",
    "DISPLAY",
    "XAUTHORITY",
)


def select_executable(cargo_json: Path) -> Path:
    candidates: list[str] = []
    for line_number, line in enumerate(cargo_json.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            message: dict[str, Any] = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"Cargo JSON line {line_number} is invalid: {error}") from error
        target = message.get("target")
        if (
            message.get("reason") == "compiler-artifact"
            and isinstance(target, dict)
            and target.get("name") == "linux_xdg_portal_smoke"
            and target.get("kind") == ["test"]
            and message.get("profile", {}).get("test") is True
            and isinstance(message.get("executable"), str)
        ):
            candidates.append(message["executable"])
    if len(candidates) != 1:
        raise ValueError(f"expected one Linux portal test executable, found {len(candidates)}")
    executable = Path(candidates[0])
    if not executable.is_absolute() or not executable.is_file():
        raise ValueError("Cargo's Linux portal test executable is not an existing absolute file")
    return executable


def run_xdotool(*args: str) -> str:
    completed = subprocess.run(
        ["xdotool", *args],
        check=True,
        capture_output=True,
        text=True,
        timeout=5,
    )
    return completed.stdout.strip()


def wait_for_stage(stage_file: Path, expected: str, deadline: float) -> None:
    while time.monotonic() < deadline:
        try:
            observed = stage_file.read_text(encoding="utf-8")
        except FileNotFoundError:
            observed = ""
        if observed == expected:
            return
        time.sleep(0.05)
    raise TimeoutError(f"test did not reach stage {expected!r}; last value was {observed!r}")


def wait_for_dialog(title: str, deadline: float) -> str:
    while time.monotonic() < deadline:
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", title],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        windows = [line for line in result.stdout.splitlines() if line.strip()]
        if result.returncode == 0 and len(windows) == 1:
            return windows[0]
        if len(windows) > 1:
            raise RuntimeError(f"multiple visible windows matched dialog title {title!r}: {windows}")
        time.sleep(0.1)
    visible = subprocess.run(
        ["xdotool", "search", "--onlyvisible", "--name", ".*"],
        capture_output=True,
        text=True,
        timeout=5,
        check=False,
    )
    names: list[str] = []
    for window in visible.stdout.splitlines():
        name = subprocess.run(
            ["xdotool", "getwindowname", window],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        if name.returncode == 0:
            names.append(name.stdout.strip())
    raise TimeoutError(f"portal dialog {title!r} did not appear; visible windows: {names}")


def wait_for_monitor_ready(monitor: subprocess.Popen[str], output: Path, timeout_seconds: float = 10) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        if monitor.poll() is not None:
            raise RuntimeError(f"D-Bus monitor exited early with status {monitor.returncode}")
        monitor_output = output.read_text(encoding="utf-8", errors="replace")
        if (
            "Monitoring the message bus" in monitor_output
            or "member=NameAcquired" in monitor_output
        ):
            return
        time.sleep(0.05)
    raise TimeoutError("D-Bus monitor did not become ready")


def capture_portal_methods(monitor: subprocess.Popen[str], output: Path) -> str:
    if monitor.poll() is not None:
        raise RuntimeError(f"D-Bus monitor exited early with status {monitor.returncode}")
    try:
        monitor.send_signal(signal.SIGINT)
    except ProcessLookupError:
        pass
    try:
        monitor.wait(timeout=5)
    except subprocess.TimeoutExpired:
        monitor.kill()
        monitor.wait(timeout=5)
        raise TimeoutError("D-Bus monitor did not stop")
    return output.read_text(encoding="utf-8", errors="replace")


def stop_owned_process(process: subprocess.Popen[str] | None) -> None:
    if process is None or process.poll() is not None:
        return
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=3)


def require_portal_calls(log: str) -> None:
    required = {
        "OpenFile": log.count("interface=org.freedesktop.portal.FileChooser; member=OpenFile"),
        "SaveFile": log.count("interface=org.freedesktop.portal.FileChooser; member=SaveFile"),
        "GTK OpenFile": log.count("interface=org.freedesktop.impl.portal.FileChooser; member=OpenFile"),
        "GTK SaveFile": log.count("interface=org.freedesktop.impl.portal.FileChooser; member=SaveFile"),
        "portal responses": log.count("interface=org.freedesktop.portal.Request; member=Response"),
    }
    if required["OpenFile"] < 2 or required["SaveFile"] < 1:
        raise AssertionError(f"rfd did not send both OpenFile and SaveFile portal requests: {required}")
    if required["GTK OpenFile"] < 2 or required["GTK SaveFile"] < 1:
        raise AssertionError(f"GTK portal backend did not receive all chooser requests: {required}")
    if required["portal responses"] < 3:
        raise AssertionError(f"XDG portal did not complete all three requests: {required}")


def emit_failure_diagnostics(root: Path) -> None:
    for name in ("portal-session.log", "portal-test.log", "dbus-monitor.log"):
        try:
            lines = (root / name).read_text(encoding="utf-8", errors="replace").splitlines()
        except OSError:
            continue
        excerpt = "\n".join(lines[-80:])
        if len(excerpt) > 12_000:
            excerpt = excerpt[-12_000:]
        if excerpt:
            print(f"--- {name} tail (max 80 lines / 12000 chars) ---\n{excerpt}", file=sys.stderr)


def prepare_staging(root: Path) -> dict[str, str]:
    config = root / "xdg-config"
    portal_config = config / "xdg-desktop-portal"
    portal_config.mkdir(parents=True)
    runtime_dir = root / "runtime"
    runtime_dir.mkdir(mode=0o700)
    runtime_dir.chmod(0o700)
    portal_preferences = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.FileChooser=gtk\n"
    (portal_config / "portals.conf").write_text(portal_preferences, encoding="utf-8")
    (portal_config / "gnome-portals.conf").write_text(portal_preferences, encoding="utf-8")
    (root / "open-fixture.txt").write_text("qc-remote-portal-open-fixture\n", encoding="utf-8")
    return {
        "QC_XDG_PORTAL_SMOKE_DIR": str(root),
        "XDG_CONFIG_HOME": str(config),
        "XDG_CURRENT_DESKTOP": "GNOME",
        "XDG_RUNTIME_DIR": str(runtime_dir),
        "XDG_SESSION_TYPE": "x11",
        "GDK_BACKEND": "x11",
    }


def unmount_document_portal(root: Path) -> None:
    mountpoint = root / "runtime" / "doc"
    if not mountpoint.is_mount():
        return
    fusermount = shutil.which("fusermount3") or shutil.which("fusermount")
    if fusermount is None:
        raise RuntimeError("document portal left a mount but fusermount is unavailable")
    result = subprocess.run(
        [fusermount, "-u", str(mountpoint)],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    # The XDG document portal is owned by the D-Bus session above. When that
    # session exits, its FUSE connection can be gone before the parent tears
    # down the mount; fusermount then reports ENOTCONN instead of detaching the
    # dead mount. Detach only for that precise condition, and still verify the
    # owned mount disappeared from this namespace.
    detail = (result.stderr or result.stdout).strip()
    if result.returncode != 0 and "Transport endpoint is not connected" in detail:
        result = subprocess.run(
            [fusermount, "-uz", str(mountpoint)],
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
    if result.returncode != 0 or mountpoint.is_mount():
        detail = (result.stderr or result.stdout).strip()
        raise RuntimeError(f"could not unmount the owned document portal mount: {detail}")


def run_session_worker(executable: Path, root: Path) -> None:
    env = os.environ.copy()
    if not (root / "xdg-config").is_dir() or not (root / "runtime").is_dir():
        raise RuntimeError("portal session staging was not prepared by the parent")
    env["QC_XDG_PORTAL_SMOKE_DIR"] = str(root)
    env["XDG_CONFIG_HOME"] = str(root / "xdg-config")
    env["XDG_CURRENT_DESKTOP"] = "GNOME"
    env["XDG_RUNTIME_DIR"] = str(root / "runtime")
    env["XDG_SESSION_TYPE"] = "x11"
    env["GDK_BACKEND"] = "x11"
    subprocess.run(
        ["dbus-update-activation-environment", *DBUS_ACTIVATION_ENVIRONMENT],
        check=True,
        capture_output=True,
        text=True,
        timeout=10,
        env=env,
    )
    monitor_log = root / "dbus-monitor.log"
    test_log = root / "portal-test.log"
    stage_file = root / "stage"
    open_fixture = root / "open-fixture.txt"
    with monitor_log.open("w", encoding="utf-8") as monitor_output, test_log.open(
        "w", encoding="utf-8"
    ) as test_output:
        monitor: subprocess.Popen[str] | None = None
        child: subprocess.Popen[str] | None = None
        try:
            monitor = subprocess.Popen(
                ["dbus-monitor", "--session"],
                stdout=monitor_output,
                stderr=subprocess.STDOUT,
                text=True,
                start_new_session=True,
            )
            wait_for_monitor_ready(monitor, monitor_log)
            child = subprocess.Popen(
                [str(executable), "--exact", TEST_NAME, "--ignored", "--nocapture"],
                env=env,
                stdout=test_output,
                stderr=subprocess.STDOUT,
                text=True,
                start_new_session=True,
            )
            deadline = time.monotonic() + MAX_TEST_SECONDS
            wait_for_stage(stage_file, "open-dialog", deadline)
            open_window = wait_for_dialog(OPEN_TITLE, min(deadline, time.monotonic() + MAX_DIALOG_SECONDS))
            run_xdotool("windowfocus", "--sync", open_window)
            run_xdotool("key", "--clearmodifiers", "ctrl+l")
            run_xdotool("type", "--clearmodifiers", "--delay", "1", str(open_fixture))
            run_xdotool("key", "Return")

            wait_for_stage(stage_file, "save-dialog", deadline)
            save_window = wait_for_dialog(SAVE_TITLE, min(deadline, time.monotonic() + MAX_DIALOG_SECONDS))
            run_xdotool("windowfocus", "--sync", save_window)
            run_xdotool("key", "Return")

            wait_for_stage(stage_file, "cancel-dialog", deadline)
            cancel_window = wait_for_dialog(CANCEL_TITLE, min(deadline, time.monotonic() + MAX_DIALOG_SECONDS))
            run_xdotool("windowfocus", "--sync", cancel_window)
            run_xdotool("key", "Escape")
            wait_for_stage(stage_file, "complete", deadline)
            try:
                exit_code = child.wait(timeout=max(0.1, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise TimeoutError("ignored Rust portal smoke test did not exit") from error
            test_output.flush()
            if exit_code != 0:
                raise RuntimeError(f"Rust portal smoke exited {exit_code}")
            portal_log = capture_portal_methods(monitor, monitor_log)
            require_portal_calls(portal_log)
        finally:
            stop_owned_process(child)
            if monitor is not None:
                if monitor.poll() is None:
                    try:
                        monitor.send_signal(signal.SIGINT)
                    except ProcessLookupError:
                        pass
                try:
                    monitor.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        monitor.kill()
                    except ProcessLookupError:
                        pass
                    monitor.wait(timeout=3)


def run_smoke(executable: Path) -> None:
    if not sys.platform.startswith("linux"):
        raise RuntimeError("the XDG portal smoke must run on Linux")
    for command in ("dbus-run-session", "xvfb-run"):
        if shutil.which(command) is None:
            raise RuntimeError(f"required portal smoke command is missing: {command}")

    with tempfile.TemporaryDirectory(prefix="qc-rfd-portal-") as temporary:
        root = Path(temporary)
        activation_env = os.environ.copy()
        activation_env.update(prepare_staging(root))
        session_log = root / "portal-session.log"
        command = [
            "dbus-run-session",
            "--",
            "xvfb-run",
            "--auto-servernum",
            "--server-args=-screen 0 1280x800x24",
            sys.executable,
            str(Path(__file__).resolve()),
            "--session-worker",
            "--staging-dir",
            str(root),
            "--test-executable",
            str(executable),
        ]
        with session_log.open("w", encoding="utf-8") as session_output:
            session: subprocess.Popen[str] | None = None
            try:
                session = subprocess.Popen(
                    command,
                    env=activation_env,
                    stdout=session_output,
                    stderr=subprocess.STDOUT,
                    text=True,
                    start_new_session=True,
                )
                try:
                    exit_code = session.wait(timeout=MAX_SESSION_SECONDS)
                except subprocess.TimeoutExpired as error:
                    stop_owned_process(session)
                    session_output.flush()
                    emit_failure_diagnostics(root)
                    raise TimeoutError("D-Bus/Xvfb portal session exceeded its deadline") from error
                session_output.flush()
                if exit_code != 0:
                    emit_failure_diagnostics(root)
                    raise RuntimeError(f"D-Bus/Xvfb portal session exited {exit_code}")
                saved = root / "saved-by-portal.txt"
                if saved.read_text(encoding="utf-8") != "qc-remote-portal-save-result\n":
                    emit_failure_diagnostics(root)
                    raise AssertionError("the selected save destination did not contain the expected payload")
                print("XDG portal GTK smoke passed: OpenFile selection, SaveFile destination, and cancellation.")
            finally:
                stop_owned_process(session)
                unmount_document_portal(root)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cargo-json", type=Path)
    parser.add_argument("--session-worker", action="store_true")
    parser.add_argument("--staging-dir", type=Path)
    parser.add_argument("--test-executable", type=Path)
    arguments = parser.parse_args()
    try:
        if arguments.session_worker:
            if arguments.staging_dir is None or arguments.test_executable is None:
                raise ValueError("session worker requires --staging-dir and --test-executable")
            run_session_worker(arguments.test_executable, arguments.staging_dir)
        else:
            if arguments.cargo_json is None:
                raise ValueError("--cargo-json is required outside session-worker mode")
            executable = select_executable(arguments.cargo_json)
            run_smoke(executable)
    except (OSError, RuntimeError, TimeoutError, ValueError, AssertionError, subprocess.SubprocessError) as error:
        print(f"Linux XDG portal smoke failed: {error}", file=sys.stderr)
        return 1
    finally:
        if arguments.cargo_json is not None:
            arguments.cargo_json.unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
