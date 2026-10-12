#!/usr/bin/env python3
"""Run the disconnected tray probe with an ephemeral Ubuntu userns profile.

Ubuntu 24.04 documents this unconfined+userns profile form in its release notes.
"""
from __future__ import annotations

import argparse
import os
import pathlib
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile

MAX_PROBE_SECONDS = 190


class ProbeInterrupted(Exception):
    def __init__(self, signum: int):
        self.signum = signum

def read_text(path: str) -> str:
    try:
        return pathlib.Path(path).read_text(encoding="utf-8").strip()
    except OSError as exc:
        return f"unavailable:{exc.__class__.__name__}"


def emit_runner_facts(bwrap: str | None) -> None:
    print(f"kernel={read_text('/proc/version')}", flush=True)
    print(f"apparmor_enabled={read_text('/sys/module/apparmor/parameters/enabled')}", flush=True)
    print(f"apparmor_restrict_unprivileged_userns={read_text('/proc/sys/kernel/apparmor_restrict_unprivileged_userns')}", flush=True)
    print(f"apparmor_profile={read_text('/proc/self/attr/current')}", flush=True)
    if bwrap:
        try:
            info = pathlib.Path(bwrap).stat()
            print(f"bwrap_path={bwrap} mode={stat.S_IMODE(info.st_mode):04o} uid={info.st_uid} gid={info.st_gid}", flush=True)
        except OSError as exc:
            print(f"bwrap_path={bwrap} stat_error={exc.__class__.__name__}", flush=True)
    else:
        print("bwrap_path=unavailable", flush=True)


def profile_text(binary: pathlib.Path) -> str:
    if not re.fullmatch(r"/[A-Za-z0-9_./-]+", str(binary)):
        raise RuntimeError("temporary bubblewrap profile path contains unsupported characters")
    return ("abi <abi/4.0>,\n\ninclude <tunables/global>\n\n"
            f"{binary} flags=(unconfined) {{\n  userns,\n}}\n")


def needs_userns_profile() -> bool:
    enabled = read_text("/sys/module/apparmor/parameters/enabled").upper().startswith("Y")
    restricted = read_text("/proc/sys/kernel/apparmor_restrict_unprivileged_userns") == "1"
    return enabled and restricted


def owned_start_ticks(pid: int) -> str | None:
    try:
        raw = pathlib.Path(f"/proc/{pid}/stat").read_text(encoding="ascii")
        fields = raw[raw.rfind(")") + 2:].split()
        return fields[19]  # proc stat field 22, starttime
    except (OSError, IndexError):
        return None


def run_owned_probe(command: list[str], env: dict[str, str]) -> int:
    proc = subprocess.Popen(command, env=env, start_new_session=True)
    identity = owned_start_ticks(proc.pid)
    if identity is None:
        proc.kill()
        proc.wait(timeout=3)
        raise RuntimeError("could not verify identity of owned probe process")
    interrupted: list[int] = []
    previous: dict[int, object] = {}

    def request_probe_cleanup(signum: int, _frame: object) -> None:
        interrupted.append(signum)
        # SIGINT gives the Python probe its normal BaseException cleanup path.
        if proc.poll() is None and owned_start_ticks(proc.pid) == identity:
            proc.send_signal(signal.SIGINT)

    for signum in (signal.SIGINT, signal.SIGTERM):
        previous[signum] = signal.signal(signum, request_probe_cleanup)
    try:
        try:
            return_code = proc.wait(timeout=MAX_PROBE_SECONDS)
        except subprocess.TimeoutExpired:
            if proc.poll() is None and owned_start_ticks(proc.pid) == identity:
                proc.send_signal(signal.SIGINT)
            try:
                return_code = proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                if owned_start_ticks(proc.pid) != identity:
                    raise RuntimeError("owned probe identity changed before timeout cleanup")
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=3)
            return 124
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
        if proc.poll() is None:
            if owned_start_ticks(proc.pid) != identity:
                raise RuntimeError("owned probe process identity changed; refusing to signal it")
            proc.send_signal(signal.SIGINT)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                if owned_start_ticks(proc.pid) != identity:
                    raise RuntimeError("owned probe identity changed before fallback cleanup")
                os.killpg(proc.pid, signal.SIGKILL)
                proc.wait(timeout=3)
    if interrupted:
        return 128 + interrupted[-1]
    return return_code


def run_probe(args: argparse.Namespace) -> int:
    bwrap = shutil.which("bwrap")
    emit_runner_facts(bwrap)
    env = os.environ.copy()
    parser = shutil.which("apparmor_parser")
    if not needs_userns_profile():
        return run_owned_probe([sys.executable, args.probe, "--broker", args.broker, "--report", args.report], env)
    if not bwrap:
        raise RuntimeError("AppArmor userns restriction is active but bubblewrap is unavailable")
    if not parser:
        raise RuntimeError("AppArmor userns restriction is active but apparmor_parser is unavailable")

    # A private copy gives this temporary profile an exact attachment path. Only
    # this probe executable receives the unconfined+userns rule; host sysctls and
    # the distro's /usr/bin/bwrap profile are untouched.
    with tempfile.TemporaryDirectory(prefix="qc-tray-bwrap-profile-", dir=os.environ.get("RUNNER_TEMP")) as temp_name:
        temp = pathlib.Path(temp_name)
        copied_bwrap = temp / "bwrap"
        shutil.copy2(bwrap, copied_bwrap)
        copied_bwrap.chmod(0o755)
        profile = temp / "profile"
        profile.write_text(profile_text(copied_bwrap), encoding="utf-8")
        loaded = False
        load_signals: list[int] = []
        load_handlers = {}
        def defer_load_signal(signum: int, _frame: object) -> None:
            load_signals.append(signum)
        try:
            for signum in (signal.SIGINT, signal.SIGTERM):
                load_handlers[signum] = signal.signal(signum, defer_load_signal)
            subprocess.run(["sudo", "-n", parser, "-r", str(profile)], check=True)
            loaded = True
            for signum, handler in load_handlers.items():
                signal.signal(signum, handler)
            load_handlers.clear()
            if load_signals:
                raise ProbeInterrupted(load_signals[-1])
            env["QC_TRAY_PROBE_BWRAP"] = str(copied_bwrap)
            return run_owned_probe([sys.executable, args.probe, "--broker", args.broker, "--report", args.report], env)
        finally:
            for signum, handler in load_handlers.items():
                signal.signal(signum, handler)
            if loaded:
                # Unload before TemporaryDirectory removes the attached executable.
                cleanup_signals: list[int] = []
                old_handlers = {}
                def defer_interrupt(signum: int, _frame: object) -> None:
                    cleanup_signals.append(signum)
                for signum in (signal.SIGINT, signal.SIGTERM):
                    old_handlers[signum] = signal.signal(signum, defer_interrupt)
                try:
                    subprocess.run(["sudo", "-n", parser, "-R", str(profile)], check=True)
                finally:
                    for signum, handler in old_handlers.items():
                        signal.signal(signum, handler)
                if cleanup_signals:
                    raise ProbeInterrupted(cleanup_signals[-1])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", default="scripts/test-linux-tray-startup.py")
    parser.add_argument("--broker", required=True)
    parser.add_argument("--report", required=True)
    args = parser.parse_args()
    def interrupted(signum: int, _frame: object) -> None:
        raise ProbeInterrupted(signum)
    previous = {signum: signal.signal(signum, interrupted) for signum in (signal.SIGINT, signal.SIGTERM)}
    try:
        return run_probe(args)
    except ProbeInterrupted as exc:
        print(f"tray-probe-interrupted=signal:{exc.signum}", file=sys.stderr, flush=True)
        return 128 + exc.signum
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"tray-probe-runner-error={exc.__class__.__name__}: {exc}", file=sys.stderr, flush=True)
        return 1
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


if __name__ == "__main__":
    raise SystemExit(main())
