import type { ConnectionState, RuntimeStatus } from "@qc-remote/client";

/**
 * `gatewayAvailable` only means the local Rust process answered. Newer
 * gateways also report the USB worker state, which lets the UI distinguish a
 * live, synchronized QC from an alive broker that is still searching.
 */
export function runtimeHasSynchronizedQc(runtime?: RuntimeStatus): boolean {
  if (!runtime?.gatewayAvailable) return false;
  const usb = runtime.usbDiagnostics;
  // Ready is fail-closed. A gateway response without live USB diagnostics is
  // only proof that the local broker process exists, not that a QC is alive.
  return Boolean(usb
    && usb.phase === "ready"
    && usb.connected
    && usb.synchronized);
}

/** Ready is deliberately stricter than the broker connection phase. */
export function qcConnectionIsVerified(
  connection: ConnectionState,
  runtime?: RuntimeStatus,
  syncProgress: number | null = null
): boolean {
  return connection.phase === "ready"
    && connection.source === "device"
    && !connection.demo
    && Boolean(connection.lastSync)
    && syncProgress === null
    && runtimeHasSynchronizedQc(runtime);
}

/**
 * A demo session is usable for editing, exactly like a verified device, but it
 * is never hardware: callers gate commands on `hardware || virtual` and gate
 * every hardware-only surface (live frames, USB recovery, backups, wording) on
 * `qcConnectionIsVerified` alone.
 */
export function qcVirtualSessionIsReady(connection: ConnectionState): boolean {
  return connection.source === "virtual" && connection.phase === "ready" && !connection.demo;
}

/**
 * Human-readable device state must be derived from the same live predicate as
 * the badge. `connection.detail` and the last diagnostics payload can briefly
 * describe the previous session while a disconnect is propagating through
 * React, so neither is allowed to claim synchronization on its own.
 */
export function qcDeviceStatusDetail(
  connection: ConnectionState,
  deviceReady: boolean,
  syncProgress: number | null = null
): string {
  if (connection.source === "virtual") {
    return connection.phase === "ready" ? "Demo device on this computer · no hardware attached" : "Demo device stopped";
  }
  if (deviceReady) return "Live and synchronized";
  if (syncProgress !== null || connection.phase === "syncing" || connection.phase === "ready") {
    return "Synchronizing active preset";
  }
  if (connection.phase === "disconnected") return "Disconnected";
  if (connection.phase === "degraded" || connection.phase === "needs-attention") return "Offline";
  if (connection.phase === "discovering") return "Searching for Quad Cortex";
  if (connection.phase === "opening" || connection.phase === "handshaking") return "Connecting";
  // "searching", and any phase this build does not name: nothing answered, so
  // nothing may be described as connecting.
  return "No Quad Cortex found";
}

/**
 * Put a native broker's connection result into the host's phase vocabulary.
 *
 * `device.reconnect` and `device.resetSession` answer with the broker's own
 * worker phase, which has members the UI union lacks. Left raw, a real USB
 * fault ("error") and a search that found nothing ("searching" or
 * "connecting") both fell through every indicator to the same calm gray, and
 * the panel called either one "Connecting". A fault is an error the user must
 * see; nothing found is the not-yet-connected idle state. The broker's own
 * detail is kept, since it names what actually happened.
 */
export { normalizeBrokerConnection } from "@qc-remote/client";

export function isQcConnectionFailure(detail: string): boolean {
  return /(?:No (?:Quad Cortex|QC) preset has been (?:synchronized|synced)|Quad Cortex is not connected|Quad Cortex connection lost|not synchronized|USB recovery|No Quad Cortex session|Gateway closed|Gateway response channel closed|communication session was cleared)/i.test(detail);
}

/** Never leave an old connection failure beside a verified green Ready state. */
export function qcVisibleStatusNotice(notice: string, deviceReady: boolean, presetLabel: string): string {
  if (deviceReady && isQcConnectionFailure(notice)) {
    return `Quad Cortex live and synchronized · ${presetLabel}`;
  }
  return notice;
}

/**
 * Broker phases that prove a Quad Cortex is actually on the other end.
 *
 * "searching" means enumeration found nothing. The broker publishes
 * "connecting" only after enumeration found a matching QC and it is opening
 * HID; "handshaking" follows once that handle is open.
 */
export function brokerPhaseDetectsDevice(phase: string): boolean {
  return phase === "connecting" || phase === "handshaking" || phase === "syncing" || phase === "ready";
}

/** Translate a live broker phase into the state shown during one connect. */
export function qcConnectionDuringBrokerPhase(
  phase: string,
  detail: string
): ConnectionState | undefined {
  if (phase === "connecting") {
    return { phase: "opening", detail: "Quad Cortex found; opening its USB connection…", demo: true, source: "device" };
  }
  if (phase === "handshaking") {
    return { phase: "handshaking", detail: detail || "USB open; handshaking with the Quad Cortex…", demo: true, source: "device" };
  }
  if (phase === "syncing" || phase === "ready") {
    return { phase: "syncing", detail: "Quad Cortex connected; synchronizing its live state…", demo: false, source: "device" };
  }
  return undefined;
}

/**
 * Whether a device the broker found while no connect() was waiting should
 * start a connection of its own. A connection already under way, a live or
 * recovering session, and a deliberate disconnect each own the next step.
 *
 * A demo session is the one exception to "ready owns the next step itself":
 * Demo Mode is the app's default, so a Quad Cortex detected while a ready
 * demo is the active view is always worth following - the same "plugged in
 * after a connect had already given up" case this function exists for,
 * except nothing else is watching for one while the demo is on screen. A
 * demo session is otherwise never in any other phase (startDemoDevice,
 * suspendDemoDevice and stopDemoDevice all route `source` back to "device"
 * the moment it stops being the active view), so there is nothing else to
 * special-case here.
 */
export function qcShouldConnectOnDetection(connection: ConnectionState, demoCanYield = true): boolean {
  if (connection.source === "virtual") return demoCanYield;
  if (["ready", "disconnected", "discovering", "opening", "handshaking", "syncing"].includes(connection.phase)) return false;
  return !connection.detail.startsWith("Live synchronization");
}

/**
 * Whether opening Demo Mode should close the broker's real-device session.
 *
 * A failed connect returns to Demo Mode while the broker deliberately keeps
 * searching. Closing that session here would disable its auto-reconnect flag,
 * so plugging the QC in later could never be detected. A ready device is also
 * retained (and surfaced as the background device). Other in-progress device
 * states reached by an explicit Demo Mode action are closed as before.
 */
export function qcShouldDisconnectForDemo(
  connection: ConnectionState,
  deviceReady: boolean,
  returningFromConnect: boolean
): boolean {
  if (connection.source !== "device" || connection.phase === "disconnected") return false;
  if (deviceReady || returningFromConnect) return false;
  return true;
}

export const SYNC_PROGRESS_START = 2;
/** Never 100 by itself: only the finished synchronization may claim that. */
export const SYNC_PROGRESS_CAP = 99;
const SYNC_PROGRESS_EASE_SPAN = 90;
const SYNC_PROGRESS_EASE_MS = 4500;
const SYNC_PROGRESS_CREEP_PER_MS = 0.2 / 1000;

/**
 * Handshake progress at `elapsedMs` after the device was detected.
 *
 * The broker cannot say how far through the boot read it is - the directory
 * size is the device's, not ours - so this is one continuous curve rather than
 * a measured fraction. It is shaped on the handshake it stands for: about 86%
 * at eleven seconds (the documented boot directory read), about 90% at
 * thirteen, and it keeps creeping by a small linear term after the ease has
 * flattened, so a slow device never leaves the bar sitting perfectly still.
 */
export function syncProgressAt(elapsedMs: number): number {
  const elapsed = Math.max(0, elapsedMs);
  const eased = SYNC_PROGRESS_EASE_SPAN * (1 - Math.exp(-elapsed / SYNC_PROGRESS_EASE_MS));
  return Math.min(SYNC_PROGRESS_CAP, SYNC_PROGRESS_START + eased + elapsed * SYNC_PROGRESS_CREEP_PER_MS);
}

/** Close the remaining gap at one steady rate, so a short gap is a short finish. */
export function syncProgressFinishMs(current: number): number {
  return Math.max(80, Math.round((100 - current) * 25));
}
