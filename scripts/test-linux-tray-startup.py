#!/usr/bin/env python3
"""Synthetic, disconnected SNI startup regression probe for a caller-supplied broker binary.

Every scenario runs in a private D-Bus session, PID namespace, network namespace, /dev,
/sys, /run, /tmp, and throwaway state directory. No USB or host device node is mounted.
"""
from __future__ import annotations
import argparse, hashlib, json, os, pathlib, shutil, signal, stat, subprocess, sys, tempfile, time, uuid
from typing import Any

MAX_TOTAL_SECONDS = 160
SYSTEM_PYTHON = "/usr/bin/python3"
DELAYED_WATCHER_SECONDS = 2.0
ABSENT_WATCHER_MIN_SECONDS = 30.0
ABSENT_WATCHER_MAX_SECONDS = 42.0
TERM_EXIT_SECONDS = 10.0

class ProbeFailure(Exception):
    def __init__(self, stage: str, code: str, cleanup_verified: bool | None = None,
                 cleanup_role: str | None = None):
        super().__init__(stage)
        self.stage, self.code = stage, code
        self.cleanup_verified, self.cleanup_role = cleanup_verified, cleanup_role

def proc_identity(pid: int) -> dict[str, Any] | None:
    try:
        root = pathlib.Path(f"/proc/{pid}")
        raw = (root / "stat").read_text()
        tail = raw[raw.rfind(")") + 2:].split()
        if tail[0] in {"Z", "X", "x"}: return None
        return {"pid": pid, "startTicks": int(tail[19]), "pgrp": int(tail[2]),
                "ppid": int(tail[1]), "exe": os.readlink(root / "exe").removesuffix(" (deleted)"),
                "cmdline": (root / "cmdline").read_bytes().replace(bytes([0]), b" ").decode(errors="replace").strip()}
    except (OSError, ValueError, IndexError):
        return None

def group_has_live_members(pgid: int) -> bool:
    try: entries = list(pathlib.Path("/proc").iterdir())
    except OSError: return True
    for entry in entries:
        if not entry.name.isdigit(): continue
        try:
            tail = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            if int(tail[2]) == pgid and tail[0] not in {"Z", "X", "x"}: return True
        except (OSError, ValueError, IndexError): pass
    return False

def wait_for_group_exit(pgid: int, grace: float = 3.0) -> bool:
    """Observe one bounded retirement interval; never signal a departed leader's PGID."""
    deadline = time.monotonic() + grace
    while group_has_live_members(pgid):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        time.sleep(min(0.05, remaining))
    return True

def group_survivor_diagnostics(pgid: int, limit: int = 16) -> dict[str, Any]:
    """Bounded process facts only: command lines may contain credentials."""
    members = []
    try:
        entries = list(pathlib.Path("/proc").iterdir())
    except OSError:
        return {"scanUnavailable": True, "members": []}
    for entry in entries:
        if not entry.name.isdigit(): continue
        try:
            raw = (entry / "stat").read_text()
            tail = raw[raw.rfind(")") + 2:].split()
            if int(tail[2]) != pgid or tail[0] in {"Z", "X", "x"}: continue
            member = {"pid": int(entry.name), "startTicks": int(tail[19]),
                      "ppid": int(tail[1]), "pgrp": int(tail[2])}
        except (OSError, ValueError, IndexError):
            continue
        if len(members) == limit:
            return {"members": members, "truncated": True}
        try: member["exe"] = os.readlink(entry / "exe")[:512]
        except OSError: member["exe"] = None
        members.append(member)
    return {"members": members, "truncated": False}

def stop_owned_group(proc: subprocess.Popen[Any], identity: dict[str, Any], grace: float = 2.0) -> bool:
    """Signal only the dedicated process group created by Popen(start_new_session=True)."""
    current = proc_identity(proc.pid)
    if current is not None and (current["startTicks"], current["pgrp"], current["exe"]) != (identity["startTicks"], proc.pid, identity["exe"]):
        return False
    if not group_has_live_members(proc.pid):
        try: proc.wait(timeout=0)
        except subprocess.TimeoutExpired: pass
        return proc.poll() is not None
    try: os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError: pass
    deadline = time.monotonic() + grace
    while time.monotonic() < deadline and group_has_live_members(proc.pid): time.sleep(0.05)
    if group_has_live_members(proc.pid):
        try: os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError: pass
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline and group_has_live_members(proc.pid): time.sleep(0.05)
    try: proc.wait(timeout=0)
    except subprocess.TimeoutExpired:
        try: proc.wait(timeout=2)
        except subprocess.TimeoutExpired: return False
    return proc.poll() is not None and not group_has_live_members(proc.pid)

def atomic_json(path: pathlib.Path, data: dict[str, Any]) -> None:
    temp = path.with_name(path.name + f".tmp-{os.getpid()}-{time.monotonic_ns()}")
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as out:
        json.dump(data, out, sort_keys=True)
        out.write("\n")
        out.flush(); os.fsync(out.fileno())
    os.replace(temp, path)
    os.chmod(path, 0o600)

def bounded_log_tail(path: pathlib.Path, limit: int = 8_000, lines: int = 30) -> str | None:
    """Read a short diagnostic tail from a probe-owned log, if available."""
    try:
        content = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    return "\n".join(content[-lines:])[-limit:] or None

def finalize_outer_report(report_path: pathlib.Path, report: dict[str, Any], staging: pathlib.Path,
                          retain_staging: bool, run_id: str) -> None:
    """Remove only our owner-verified staging tree, then publish truthful cleanup state."""
    report["stagingRemoved"] = False
    try:
        if retain_staging:
            report["stagingRemoved"] = not staging.exists() and not staging.is_symlink()
            if not report["stagingRemoved"]: report["stagingPathPreserved"] = str(staging)
        elif staging.exists() or staging.is_symlink():
            metadata = staging.lstat()
            if (staging.is_symlink() or not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid()
                    or stat.S_IMODE(metadata.st_mode) != 0o700 or not staging.name.startswith(f"qc-tray-startup-{run_id}-")):
                raise OSError("staging identity changed; refusing recursive cleanup")
            shutil.rmtree(staging)
            report["stagingRemoved"] = not staging.exists() and not staging.is_symlink()
        else:
            report["stagingRemoved"] = True
    except OSError:
        report["stagingRemoved"] = not staging.exists() and not staging.is_symlink()
        report["stagingCleanupFailure"] = "OwnedStagingRemovalFailed"
        if not report["stagingRemoved"]: report["stagingPathPreserved"] = str(staging)
    inner_cleanup_verified = (report.get("innerCleanupVerified") is True
                              or report.get("innerStarted") is False)
    report["cleanupVerified"] = (report.get("sandboxGroupReaped") is True
                                  and inner_cleanup_verified
                                  and report["stagingRemoved"] is True)
    report["ok"] = report.get("ok") is True and report["cleanupVerified"]
    if not report["cleanupVerified"] and "failureStage" not in report:
        report["failureStage"], report["failureCode"] = "cleanup", "OwnedCleanupUnverified"
    report_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    atomic_json(report_path, report)

WATCHER_XML = '''<node>
 <interface name="org.kde.StatusNotifierWatcher">
  <method name="RegisterStatusNotifierItem"><arg name="service" type="s" direction="in"/></method>
  <method name="RegisterStatusNotifierHost"><arg name="service" type="s" direction="in"/></method>
  <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
  <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
  <property name="ProtocolVersion" type="i" access="read"/>
  <signal name="StatusNotifierItemRegistered"><arg name="service" type="s"/></signal>
  <signal name="StatusNotifierItemUnregistered"><arg name="service" type="s"/></signal>
 </interface>
</node>'''

def watcher_main(ready_path: pathlib.Path, registered_path: pathlib.Path, host_registered: bool) -> int:
    """Expose the real SNI watcher bus name/method/properties on this private bus."""
    import gi
    from gi.repository import Gio, GLib
    connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    items: list[str] = []
    loop = GLib.MainLoop()
    info = Gio.DBusNodeInfo.new_for_xml(WATCHER_XML).interfaces[0]
    def method_call(conn, sender, obj_path, interface, method, params, invocation):
        values = params.unpack()
        if method == "RegisterStatusNotifierItem":
            service = str(values[0])
            if service.startswith("/"):
                item = sender + service
            else:
                item = service + "/StatusNotifierItem"
            if item not in items: items.append(item)
            conn.emit_signal(None, "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher", "StatusNotifierItemRegistered", GLib.Variant("(s)", (item,)))
            atomic_json(registered_path, {"registered": True, "itemCount": len(items), "protocolMethod": method})
            invocation.return_value(GLib.Variant("()", ()))
        elif method == "RegisterStatusNotifierHost":
            invocation.return_value(GLib.Variant("()", ()))
        else:
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unsupported watcher method")
    def get_property(_conn, _sender, _path, _interface, prop):
        if prop == "RegisteredStatusNotifierItems": return GLib.Variant("as", items)
        if prop == "IsStatusNotifierHostRegistered": return GLib.Variant("b", host_registered)
        if prop == "ProtocolVersion": return GLib.Variant("i", 0)
        return None
    registration = connection.register_object("/StatusNotifierWatcher", info, method_call, get_property, None)
    if not registration: return 4
    request = connection.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "RequestName",
        GLib.Variant("(su)", ("org.kde.StatusNotifierWatcher", 0)), GLib.VariantType.new("(u)"), Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]
    if request not in (1, 4):
        connection.unregister_object(registration)
        return 3
    atomic_json(ready_path, {"ready": True, "watcherServiceOwned": True, "hostRegistered": host_registered})
    try: loop.run()
    finally: connection.unregister_object(registration)
    return 0

def marker_paths(state: pathlib.Path) -> list[pathlib.Path]:
    return [state / "local-mcp.json", state / "device-broker.json", state / "device-broker-resident.json"]

def claims_absent(state: pathlib.Path) -> bool:
    # This directory is created exclusively for one broker invocation. Check
    # the production descriptor/claim and election/claim names, plus the
    # descriptor publication's known staging-file shape. The three JSON
    # markers alone missed the OS-locked sidecars that matter to ownership.
    owned = [*marker_paths(state)]
    owned.extend(state / name for name in (
        "device-broker.json.claim", "device-broker.election", "device-broker.election.claim"))
    if any(path.exists() or path.is_symlink() for path in owned): return False
    try: entries = list(state.iterdir())
    except FileNotFoundError: return True
    except OSError: return False
    return not any((entry.name.startswith("device-broker.json.") and entry.name.endswith(".tmp"))
                   or (entry.name.startswith("device-broker.election.") and entry.name.endswith(".tmp"))
                   for entry in entries)

def query_watcher_host_registered() -> bool:
    """Read IsStatusNotifierHostRegistered from the live private-bus service."""
    import gi
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    reply = bus.call_sync("org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
        "org.freedesktop.DBus.Properties", "Get",
        GLib.Variant("(ss)", ("org.kde.StatusNotifierWatcher", "IsStatusNotifierHostRegistered")),
        GLib.VariantType.new("(v)"), Gio.DBusCallFlags.NONE, 1500, None)
    value = reply.unpack()[0]
    # Gio versions differ on whether the (v) payload is unwrapped by unpack.
    if isinstance(value, GLib.Variant): value = value.unpack()
    if not isinstance(value, bool): raise ProbeFailure("watcher_property", "HostPropertyWasNotBoolean")
    return value

def start_child(argv: list[str], env: dict[str, str], log_path: pathlib.Path,
                role: str = "broker") -> tuple[subprocess.Popen[Any], dict[str, Any]]:
    with log_path.open("ab") as log:
        proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env,
                                cwd=log_path.parent, start_new_session=True, close_fds=True)
    identity = proc_identity(proc.pid)
    if identity is None or identity["pgrp"] != proc.pid:
        # Never signal a process group when its owner identity was not proved.
        # The enclosing bubblewrap PID namespace will be torn down on failure;
        # the case must still report inner cleanup as unverified.
        raise ProbeFailure("child_start", "OwnedProcessIdentityUnavailable",
                           cleanup_verified=False, cleanup_role=role)
    return proc, identity

def wait_file(path: pathlib.Path, proc: subprocess.Popen[Any], deadline: float) -> bool:
    while time.monotonic() < deadline:
        if path.is_file(): return True
        if proc.poll() is not None: return False
        time.sleep(0.05)
    return path.is_file()

def wait_for_descriptor(state: pathlib.Path, broker: subprocess.Popen[Any], expected_pid: int,
                        deadline: float) -> bool:
    path = state / "device-broker.json"
    while time.monotonic() < deadline:
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
            if record.get("processId") == expected_pid: return True
        except (OSError, ValueError, AttributeError): pass
        if broker.poll() is not None: return False
        time.sleep(0.05)
    return False

def launch_broker_case(binary: str, state: pathlib.Path, env: dict[str, str], log: pathlib.Path):
    state.mkdir(mode=0o700, parents=True)
    proc, identity = start_child([binary, "--resident", "--directory", str(state), "--keep-user-quit"], env, log)
    if identity["exe"] != os.path.realpath(binary):
        cleaned = stop_owned_group(proc, identity, grace=1)
        raise ProbeFailure("broker_identity", "ExecutableMismatch" if cleaned else "ExecutableMismatchCleanupUnverified",
                           cleanup_verified=cleaned, cleanup_role="broker")
    return proc, identity

def stop_naturally(proc: subprocess.Popen[Any], identity: dict[str, Any], seconds: float) -> bool:
    if proc_identity(proc.pid) != identity: return False
    os.kill(proc.pid, signal.SIGTERM)
    try: proc.wait(timeout=seconds)
    except subprocess.TimeoutExpired: return False
    return proc.returncode == 0 and not group_has_live_members(proc.pid)

def disconnected_checks() -> dict[str, Any]:
    mounts = set()
    for line in pathlib.Path("/proc/self/mountinfo").read_text().splitlines():
        fields = line.split(); sep = fields.index("-"); mounts.add((fields[4], fields[sep + 1]))
    usb_paths = (pathlib.Path("/dev/bus/usb"), pathlib.Path("/dev/hidraw*"), pathlib.Path("/dev/snd"), pathlib.Path("/sys/bus/usb/devices"))
    import glob
    visible = bool(glob.glob("/dev/bus/usb/**/*", recursive=True) or glob.glob("/dev/hidraw*") or glob.glob("/dev/snd/**/*", recursive=True) or glob.glob("/sys/bus/usb/devices/*") or pathlib.Path("/proc/bus/usb/devices").exists())
    parent_pid_ns = os.environ.get("QC_PARENT_PID_NS", "")
    parent_net_ns = os.environ.get("QC_PARENT_NET_NS", "")
    private_pid = bool(parent_pid_ns) and os.readlink("/proc/self/ns/pid") != parent_pid_ns
    private_net = bool(parent_net_ns) and os.readlink("/proc/self/ns/net") != parent_net_ns
    return {"privateDev": ("/dev", "tmpfs") in mounts, "privateSys": ("/sys", "tmpfs") in mounts,
            "privatePidNamespace": private_pid, "privateNetworkNamespace": private_net,
            "usbOrHidNodesVisible": visible}

def run_case(name: str, binary: str, base: pathlib.Path, env: dict[str, str]) -> dict[str, Any]:
    result: dict[str, Any] = {"name": name, "passed": False, "claimsRemoved": False, "ownedBrokerGroupReaped": False, "watcherGroupReaped": True}
    state = base / f"state-{name}"; watcher_proc = watcher_identity = broker_proc = broker_identity = None
    broker_started_at: float | None = None
    safe_to_remove_state = False
    try:
        isolation = disconnected_checks(); result["isolation"] = isolation
        if not isolation["privateDev"] or not isolation["privateSys"] or not isolation["privatePidNamespace"] or not isolation["privateNetworkNamespace"] or isolation["usbOrHidNodesVisible"]:
            raise ProbeFailure("device_isolation", "PrivateDeviceIsolationFailed")
        # The no-host case must bring up the watcher first so the broker sees
        # a real watcher that ACKs registration while reporting no host.
        if name != "watcher-without-host":
            broker_proc, broker_identity = launch_broker_case(binary, state, env, base / f"broker-{name}.log")
            broker_started_at = time.monotonic()
            result["brokerPid"] = broker_identity["pid"]
            result["brokerStartTicks"] = broker_identity["startTicks"]
            result["brokerExe"] = broker_identity["exe"]
            if not wait_for_descriptor(state, broker_proc, broker_proc.pid, time.monotonic() + 8):
                raise ProbeFailure("broker_descriptor", "BrokerDescriptorNotPublished")
            result["brokerDescriptorPublished"] = True
        if name == "delayed-watcher":
            time.sleep(DELAYED_WATCHER_SECONDS)
            ready, registered = base / "watcher-ready.json", base / "watcher-registered.json"
            watcher_proc, watcher_identity = start_child([SYSTEM_PYTHON, str(pathlib.Path(__file__).resolve()), "--watcher", str(ready), str(registered), "true"], env, base / "watcher-delayed.log", role="watcher")
            if not wait_file(ready, watcher_proc, time.monotonic() + 5): raise ProbeFailure("watcher_ready", "SyntheticWatcherDidNotOwnName")
            if not wait_file(registered, broker_proc, time.monotonic() + 12): raise ProbeFailure("delayed_register", "BrokerDidNotRegisterAfterWatcherAppeared")
            try:
                descriptor = json.loads((state / "device-broker.json").read_text())
            except Exception: raise ProbeFailure("delayed_claims", "ResidentClaimsMissingOrInvalid") from None
            if descriptor.get("processId") != broker_proc.pid:
                raise ProbeFailure("delayed_claims", "ResidentClaimsWrongProcess")
            # The standalone broker binary intentionally has no background
            # ResidentHost, so the optional host session marker need not exist.
            resident = state / "device-broker-resident.json"
            if resident.exists():
                try: marker = json.loads(resident.read_text())
                except Exception: raise ProbeFailure("delayed_claims", "ResidentMarkerInvalid") from None
                if marker.get("pid") != broker_proc.pid:
                    raise ProbeFailure("delayed_claims", "ResidentMarkerWrongProcess")
            if proc_identity(broker_proc.pid) != broker_identity: raise ProbeFailure("delayed_resident", "BrokerExitedOrIdentityChanged")
            result["registeredAfterDelaySeconds"] = DELAYED_WATCHER_SECONDS
            result["descriptorStillOwnedAfterDelayedRegistration"] = True
            if not stop_naturally(broker_proc, broker_identity, TERM_EXIT_SECONDS): raise ProbeFailure("delayed_shutdown", "BrokerDidNotCleanAfterTerm")
            result["cleanTermExit"] = True
        elif name == "watcher-absent":
            started = broker_started_at
            if started is None: raise ProbeFailure("absent_timer", "BrokerStartTimeMissing")
            deadline = started + ABSENT_WATCHER_MAX_SECONDS
            while time.monotonic() < deadline and broker_proc.poll() is None: time.sleep(0.1)
            result["pendingSeconds"] = round(time.monotonic() - started, 3)
            result["exitCode"] = broker_proc.poll()
            if result["pendingSeconds"] < ABSENT_WATCHER_MIN_SECONDS: raise ProbeFailure("absent_timeout", "BrokerRetiredBeforeThirtySeconds")
            if broker_proc.poll() is None: raise ProbeFailure("absent_timeout", "BrokerDidNotRetireWithinBound")
            if broker_proc.returncode != 0: raise ProbeFailure("absent_exit", "BrokerRetirementWasNotClean")
            result["retiredWithoutWatcher"] = True
        elif name == "watcher-without-host":
            ready, registered = base / "watcher-nohost-ready.json", base / "watcher-nohost-registered.json"
            watcher_proc, watcher_identity = start_child([SYSTEM_PYTHON, str(pathlib.Path(__file__).resolve()), "--watcher", str(ready), str(registered), "false"], env, base / "watcher-nohost.log", role="watcher")
            if not wait_file(ready, watcher_proc, time.monotonic() + 5): raise ProbeFailure("nohost_watcher_ready", "SyntheticWatcherDidNotOwnName")
            if query_watcher_host_registered() is not False: raise ProbeFailure("nohost_property", "HostPropertyWasNotFalseBeforeBroker")
            broker_proc, broker_identity = launch_broker_case(binary, state, env, base / f"broker-{name}.log")
            broker_started_at = time.monotonic()
            result["brokerPid"] = broker_identity["pid"]
            result["brokerStartTicks"] = broker_identity["startTicks"]
            result["brokerExe"] = broker_identity["exe"]
            if not wait_for_descriptor(state, broker_proc, broker_proc.pid, time.monotonic() + 8):
                raise ProbeFailure("broker_descriptor", "BrokerDescriptorNotPublished")
            result["brokerDescriptorPublished"] = True
            if not wait_file(registered, broker_proc, time.monotonic() + 12): raise ProbeFailure("nohost_ack", "WatcherDidNotAcknowledgeItemRegistration")
            result["watcherAcknowledgedRegistration"] = True
            if query_watcher_host_registered() is not False: raise ProbeFailure("nohost_property", "HostPropertyChangedAfterRegistration")
            started = broker_started_at
            if started is None: raise ProbeFailure("nohost_timer", "BrokerStartTimeMissing")
            deadline = started + ABSENT_WATCHER_MAX_SECONDS
            samples = 0
            while time.monotonic() < deadline and broker_proc.poll() is None:
                if query_watcher_host_registered() is not False: raise ProbeFailure("nohost_property", "HostPropertyBecameTrueDuringPending")
                samples += 1
                time.sleep(0.25)
            result["pendingSeconds"] = round(time.monotonic() - started, 3)
            result["hostFalseSamples"] = samples
            result["exitCode"] = broker_proc.poll()
            if result["pendingSeconds"] < ABSENT_WATCHER_MIN_SECONDS: raise ProbeFailure("nohost_timeout", "BrokerRetiredBeforeThirtySeconds")
            if broker_proc.poll() is None: raise ProbeFailure("nohost_timeout", "BrokerDidNotRetireWithinBound")
            if broker_proc.returncode != 0: raise ProbeFailure("nohost_exit", "BrokerRetirementWasNotClean")
            result["retiredWithWatcherButNoHost"] = True
        elif name == "term-pending":
            time.sleep(0.75)
            if broker_proc.poll() is not None: raise ProbeFailure("term_pending", "BrokerExitedBeforeSignal")
            if proc_identity(broker_proc.pid) != broker_identity: raise ProbeFailure("term_pending", "BrokerIdentityChanged")
            if not claims_absent(state): result["claimsBeforeTerm"] = [p.name for p in marker_paths(state) if p.exists()]
            os.kill(broker_proc.pid, signal.SIGTERM)
            try: broker_proc.wait(timeout=TERM_EXIT_SECONDS)
            except subprocess.TimeoutExpired: raise ProbeFailure("term_exit", "BrokerIgnoredTermDuringPendingStartup") from None
            result["exitCode"] = broker_proc.returncode
            if broker_proc.returncode != 0: raise ProbeFailure("term_exit", "BrokerTermExitWasNotClean")
            result["cleanTermExit"] = True
        else: raise ProbeFailure("scenario", "UnknownScenario")
        result["claimsRemoved"] = claims_absent(state)
        if not result["claimsRemoved"]: raise ProbeFailure("cleanup_claims", "DescriptorClaimsRemain")
        result["ownedBrokerGroupReaped"] = not group_has_live_members(broker_proc.pid)
        if not result["ownedBrokerGroupReaped"]: raise ProbeFailure("cleanup_broker", "OwnedBrokerChildRemains")
        result["passed"] = True
    except ProbeFailure as error:
        result["failureStage"], result["failureCode"] = error.stage, error.code
        if error.cleanup_verified is False:
            if error.cleanup_role == "watcher": result["watcherGroupReaped"] = False
            else: result["ownedBrokerGroupReaped"] = False
    except Exception as error:
        result["failureStage"], result["failureCode"] = "unexpected", type(error).__name__
    finally:
        if broker_proc is not None and broker_identity is not None:
            if broker_proc.poll() is None or group_has_live_members(broker_proc.pid):
                result["ownedBrokerGroupReaped"] = stop_owned_group(broker_proc, broker_identity, grace=2)
            else:
                try: broker_proc.wait(timeout=0)
                except subprocess.TimeoutExpired: pass
                result["ownedBrokerGroupReaped"] = not group_has_live_members(broker_proc.pid)
        else:
            result["ownedBrokerGroupReaped"] = result["ownedBrokerGroupReaped"] is not False
        if watcher_proc is not None and watcher_identity is not None:
            result["watcherGroupReaped"] = stop_owned_group(watcher_proc, watcher_identity, grace=1)
        else:
            result["watcherGroupReaped"] = result["watcherGroupReaped"] is not False
        if not result["passed"]:
            log_path = base / f"broker-{name}.log"
            try:
                # Keep concise process diagnostics in the private report before
                # the verified staging tree is removed, so harness failures
                # remain diagnosable without retaining logs or binaries.
                lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
                if lines: result["brokerLogTail"] = "\n".join(lines[-30:])[-8000:]
            except OSError: pass
        result["claimsRemoved"] = claims_absent(state)
        safe_to_remove_state = result["ownedBrokerGroupReaped"] and result["watcherGroupReaped"] and result["claimsRemoved"]
        if state.exists() and safe_to_remove_state: shutil.rmtree(state)
        result["fixtureStateRemoved"] = not state.exists()
        result["cleanupVerified"] = safe_to_remove_state and result["fixtureStateRemoved"]
        result["passed"] = result["passed"] and result["cleanupVerified"]
    return result

def inner_main(binary: str, workdir: pathlib.Path, report_path: pathlib.Path,
               path_check_only: bool = False) -> int:
    # The outer owner uses this marker to distinguish bwrap setup failure from
    # an inner run whose resources need their own cleanup verification.
    atomic_json(workdir / "inner-started.json", {"started": True})
    report: dict[str, Any] = {"syntheticSniFixture": True, "hardwareAccessed": False,
        "privateDbus": os.environ.get("DBUS_SESSION_BUS_ADDRESS", "").startswith("unix:"),
        "cases": [], "cleanupVerified": False}
    env = os.environ.copy()
    env.update({"HOME": str(workdir / "home"), "DISPLAY": ":99", "GDK_BACKEND": "x11",
                "XDG_RUNTIME_DIR": str(workdir / "runtime"), "XDG_STATE_HOME": str(workdir / "xdg-state"),
                "XDG_CONFIG_HOME": str(workdir / "xdg-config"), "XDG_CACHE_HOME": str(workdir / "xdg-cache"),
                "WEBKIT_DISABLE_COMPOSITING_MODE": "1"})
    for key in ("HOME", "XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"):
        pathlib.Path(env[key]).mkdir(mode=0o700, parents=True, exist_ok=True)
    fixture_directory = workdir / "path-owner-fixture"
    fixture_directory.mkdir(mode=0o700)
    path_chain = (pathlib.Path("/"), pathlib.Path("/run"), workdir, fixture_directory)
    uid = os.geteuid()
    snapshot: dict[str, dict[str, int]] = {}
    path_safe = True
    for path in path_chain:
        metadata = path.lstat()
        snapshot[str(path)] = {"uid": metadata.st_uid, "mode": stat.S_IMODE(metadata.st_mode)}
        path_safe = path_safe and stat.S_ISDIR(metadata.st_mode) and not stat.S_ISLNK(metadata.st_mode)
        path_safe = path_safe and metadata.st_uid in (uid, 0) and metadata.st_mode & 0o022 == 0
    path_safe = path_safe and snapshot[str(fixture_directory)]["mode"] == 0o700
    fixture_directory.rmdir()
    report["trustedPathOwners"] = snapshot
    report["effectiveUid"] = uid
    report["pathSafetyFixturePassed"] = path_safe
    if not path_safe:
        report["failureStage"], report["failureCode"] = "path_preflight", "PrivatePathOwnershipRejected"
        report["cleanupVerified"] = True
        atomic_json(report_path, report)
        return 2
    if path_check_only:
        report["cases"] = []
        report["cleanupVerified"] = True
        report["ok"] = True
        atomic_json(report_path, report)
        return 0
    isolation = disconnected_checks(); report["isolation"] = isolation
    if not report["privateDbus"] or not isolation["privateDev"] or not isolation["privateSys"] or not isolation["privatePidNamespace"] or not isolation["privateNetworkNamespace"] or isolation["usbOrHidNodesVisible"]:
        report["failureStage"], report["failureCode"] = "sandbox", "IsolationPrerequisiteFailed"
        report["cleanupVerified"] = True  # no fixture process or case state was created
        atomic_json(report_path, report); return 2
    try:
        for name in ("delayed-watcher", "watcher-absent", "watcher-without-host", "term-pending"):
            result = run_case(name, binary, workdir, env)
            report["cases"].append(result)
            if not result.get("cleanupVerified"): break
        report["cleanupVerified"] = all(x.get("cleanupVerified") for x in report["cases"])
        report["ok"] = len(report["cases"]) == 4 and report["cleanupVerified"] and all(x.get("passed") for x in report["cases"])
        if not report["ok"]:
            failed = next((c for c in report["cases"] if not c.get("passed")), {})
            report["failureStage"] = failed.get("failureStage", "case")
            report["failureCode"] = failed.get("failureCode", "CaseFailed")
    except Exception as error:
        report.update(ok=False, failureStage="inner", failureCode=type(error).__name__)
    atomic_json(report_path, report)
    return 0 if report.get("ok") is True else 1

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--broker", type=pathlib.Path, help="caller-specified executable (required in outer mode)")
    parser.add_argument("--report", type=pathlib.Path, help="final JSON report path (outer mode)")
    parser.add_argument("--path-check-only", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--inner", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--workdir", type=pathlib.Path, help=argparse.SUPPRESS)
    parser.add_argument("--report-path", type=pathlib.Path, help=argparse.SUPPRESS)
    parser.add_argument("--inner-path-check-only", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--watcher", nargs=3, metavar=("READY_JSON", "REGISTERED_JSON", "HOST_REGISTERED"), help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.watcher:
        return watcher_main(pathlib.Path(args.watcher[0]), pathlib.Path(args.watcher[1]), args.watcher[2].lower() == "true")
    if args.inner:
        if not args.workdir or not args.report_path: parser.error("inner invocation requires workdir and report-path")
        return inner_main(str(args.broker), args.workdir, args.report_path, args.inner_path_check_only)
    if os.geteuid() == 0: parser.error("run as an unprivileged test user, never root")
    if args.broker is None or args.report is None: parser.error("--broker and --report are required")
    source = args.broker.resolve(strict=True)
    if not source.is_file() or not os.access(source, os.X_OK): parser.error("--broker must resolve to an executable regular file")
    if not pathlib.Path(SYSTEM_PYTHON).is_file() or not os.access(SYSTEM_PYTHON, os.X_OK):
        parser.error(f"required system interpreter unavailable: {SYSTEM_PYTHON}")
    binary_sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
    bwrap = os.environ.get("QC_TRAY_PROBE_BWRAP") or shutil.which("bwrap")
    runid = uuid.uuid4().hex
    temp = pathlib.Path(tempfile.mkdtemp(prefix=f"qc-tray-startup-{runid}-", dir="/var/tmp"))
    sandbox_binary = "/run/qc-tray-probe/bin/qc-device-broker"
    result_path = temp / "result.json"
    script = pathlib.Path(__file__).resolve()
    report: dict[str, Any] = {"schemaVersion": 1, "syntheticSniFixture": True, "hardwareAccessed": False,
                              "brokerPath": str(source), "brokerSha256": binary_sha256,
                              "runId": runid, "overallDeadlineSeconds": MAX_TOTAL_SECONDS,
                              "cleanupVerified": False}
    proc = None; outer_clean = True; keep_temp = False; sandbox_log = None
    try:
        os.chmod(temp, 0o700)
        staged_bin = temp / "bin"
        staged_bin.mkdir(mode=0o700)
        staged_target = staged_bin / "qc-device-broker"
        staged_target.touch(mode=0o700)
        if not bwrap: raise ProbeFailure("preflight", "BubblewrapUnavailable")
        # Start with a user-owned tmpfs root rather than bind-mounting the host
        # root. In an unprivileged user namespace the latter maps host root to the
        # overflow uid (65534), which the broker correctly rejects as a foreign
        # directory owner. Bind only read-only runtime roots through pre-opened
        # FDs; this also avoids resolving host paths after the private root exists.
        # Explicitly create a user namespace before the private network namespace.
        # On Ubuntu 24.04, AppArmor may still deny namespace capabilities unless
        # the probe-only bubblewrap executable has a narrowly scoped userns rule.
        # Loopback configuration remains entirely inside this private namespace.
        bind_sources = ["/usr", "/etc", str(temp), str(source), str(script)]
        bind_fds: list[int] = []
        for source_path in bind_sources:
            fd = os.open(source_path, getattr(os, "O_PATH", os.O_RDONLY) | os.O_CLOEXEC)
            bind_fds.append(fd)
        argv = [bwrap, "--die-with-parent", "--unshare-user", "--unshare-pid", "--unshare-net", "--tmpfs", "/",
                "--dir", "/usr", "--ro-bind-fd", str(bind_fds[0]), "/usr",
                "--dir", "/etc", "--ro-bind-fd", str(bind_fds[1]), "/etc",
                "--symlink", "usr/bin", "/bin", "--symlink", "usr/sbin", "/sbin",
                "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib64", "/lib64",
                "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/sys", "--tmpfs", "/run", "--tmpfs", "/tmp",
                "--dir", "/run/qc-tray-probe", "--bind-fd", str(bind_fds[2]), "/run/qc-tray-probe",
                "--ro-bind-fd", str(bind_fds[3]), sandbox_binary,
                "--ro-bind-fd", str(bind_fds[4]), "/run/qc-tray-probe/tray-startup-runtime-probe.py",
                "--chdir", "/run/qc-tray-probe", "--clearenv", "--setenv", "PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "--setenv", "LANG", "C.UTF-8", "--setenv", "DISPLAY", ":99", "--setenv", "GDK_BACKEND", "x11",
                "--setenv", "WEBKIT_DISABLE_COMPOSITING_MODE", "1",
                "--setenv", "QC_PARENT_PID_NS", os.readlink("/proc/self/ns/pid"),
                "--setenv", "QC_PARENT_NET_NS", os.readlink("/proc/self/ns/net"),
                "--", "/usr/bin/dbus-run-session", "--", SYSTEM_PYTHON,
                "/run/qc-tray-probe/tray-startup-runtime-probe.py", "--inner", "--broker", sandbox_binary, "--workdir", "/run/qc-tray-probe", "--report-path", "/run/qc-tray-probe/result.json"]
        if args.path_check_only: argv.append("--inner-path-check-only")
        try:
            sandbox_log = (temp / "sandbox.log").open("ab")
            proc = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                    stderr=sandbox_log, start_new_session=True,
                                    pass_fds=tuple(bind_fds))
        finally:
            for fd in bind_fds:
                os.close(fd)
        outer_identity = proc_identity(proc.pid)
        report["sandboxPid"] = proc.pid
        report["sandboxStartTicks"] = outer_identity["startTicks"] if outer_identity else None
        try: proc.wait(timeout=MAX_TOTAL_SECONDS)
        except subprocess.TimeoutExpired:
            report["failureStage"], report["failureCode"] = "overall_deadline", "ProbeTimedOut"
        if sandbox_log is not None:
            sandbox_log.close(); sandbox_log = None
        if proc.poll() is None:
            current = proc_identity(proc.pid)
            if current is None or outer_identity is None or current["startTicks"] != outer_identity["startTicks"]:
                outer_clean = False
            else:
                outer_clean = stop_owned_group(proc, outer_identity, grace=3)
        else:
            try: proc.wait(timeout=0)
            except subprocess.TimeoutExpired: pass
            # The namespace launcher can exit before its remaining monitors
            # finish retiring. Observe that teardown without signalling a
            # numeric process group whose original leader has already exited.
            outer_clean = wait_for_group_exit(proc.pid)
        if not outer_clean:
            report["sandboxGroupSurvivors"] = group_survivor_diagnostics(proc.pid)
        report["sandboxGroupReaped"] = outer_clean
        if result_path.is_file():
            inner = json.loads(result_path.read_text(encoding="utf-8"))
            report["inner"] = inner
            report["innerCleanupVerified"] = inner.get("cleanupVerified") is True
            report["ok"] = proc.returncode == 0 and outer_clean and report["innerCleanupVerified"] and inner.get("ok") is True
            if not report["ok"]:
                report.setdefault("failureStage", inner.get("failureStage", "sandbox_exit"))
                report.setdefault("failureCode", inner.get("failureCode", f"SandboxExit{proc.returncode}"))
        else:
            report.setdefault("failureStage", "sandbox_report")
            report.setdefault("failureCode", "ReportMissing")
        keep_temp = not (outer_clean and report.get("innerCleanupVerified") is True)
    except ProbeFailure as error:
        report["failureStage"], report["failureCode"] = error.stage, error.code
        report["sandboxGroupReaped"] = outer_clean
        if proc is None: report["innerCleanupVerified"] = True
        keep_temp = not (outer_clean and report.get("innerCleanupVerified") is True)
    except BaseException as error:
        report["failureStage"], report["failureCode"] = "outer_exception", type(error).__name__
        if proc is not None and proc.poll() is None and (outer_identity := proc_identity(proc.pid)) is not None:
            outer_clean = stop_owned_group(proc, outer_identity, grace=3)
        report["sandboxGroupReaped"] = outer_clean
        if proc is None: report["innerCleanupVerified"] = True
        keep_temp = not (outer_clean and report.get("innerCleanupVerified") is True)
    finally:
        if sandbox_log is not None: sandbox_log.close()
        inner_started = (temp / "inner-started.json").is_file()
        report["innerStarted"] = inner_started
        # If the child private namespace has no live members and the inner entry
        # point never began, no fixture-owned broker or watcher resources could
        # have been created; remove only this verified run directory.
        keep_temp = not (outer_clean and (not inner_started or report.get("innerCleanupVerified") is True))
        if report.get("ok") is not True:
            tail = bounded_log_tail(temp / "sandbox.log")
            if tail is not None: report["sandboxStderrTail"] = tail
        finalize_outer_report(args.report, report, temp, keep_temp, runid)
    return 0 if report.get("ok") is True else 1

if __name__ == "__main__": raise SystemExit(main())
