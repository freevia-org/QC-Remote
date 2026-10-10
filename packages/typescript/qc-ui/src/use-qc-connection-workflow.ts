import { useCallback, useEffect, useMemo, useState, type Dispatch, type SetStateAction } from "react";
import type { ConnectionPhase, ConnectionState } from "@qc-remote/client";
import type { PublicRelayDevice, PublicRelayStatus } from "@qc-remote/core";

export type QcConnectionTransition = "absent" | "available" | "connecting" | "syncing" | "connected" | "error";

/**
 * Shared indicator meaning across both native hosts.
 *
 * `demo` is the local virtual device: fully editable, never hardware. It has
 * its own hue so it can be mistaken neither for a live Quad Cortex (`ready`)
 * nor for nothing being connected (`idle`).
 */
export type QcIndicatorPhase = "idle" | "available" | "syncing" | "ready" | "demo" | "action-required" | "error";

export type QcDeviceIndicatorCondition = {
  /** The USB session still reads, but writes are paused until the user resets
   * or reconnects it. Mirrors the tray's amber quarantine state. */
  actionRequired?: boolean;
  /** A host/UI failure independent of the device transport phase. */
  error?: boolean;
};

/** Keep observing a live device session even when its controls are suspended
 * or a bulk operation temporarily blocks commands. Those state frames carry
 * the terminal event that releases the block. */
export function qcLiveSessionCanObserveState(connection: ConnectionState): boolean {
  return connection.source === "device"
    && !connection.demo
    && ["handshaking", "syncing", "ready"].includes(connection.phase);
}

/** A replaced producer cannot finish transient work owned by its predecessor. */
export function qcStreamRestartClearsBulkOperation(reason: "initial" | "restarted" | "gap"): boolean {
  return reason === "restarted";
}

const transitionPhase: Record<QcConnectionTransition, ConnectionPhase> = {
  absent: "disconnected",
  available: "discovering",
  connecting: "opening",
  syncing: "syncing",
  connected: "ready",
  error: "needs-attention"
};

const transitionDetail: Record<QcConnectionTransition, string> = {
  absent: "Quad Cortex is not connected.",
  available: "Quad Cortex USB device is available.",
  connecting: "Opening the Quad Cortex USB session…",
  syncing: "Quad Cortex connected; synchronizing live state…",
  connected: "Quad Cortex connected.",
  error: "The Quad Cortex connection needs attention."
};

export interface QcConnectionPresentation {
  connected: boolean;
  busy: boolean;
  label: "USB" | "SYNC" | "WAIT" | "CONNECT";
  appearance: QcConnectionTransition;
}

export function qcConnectionPresentation(connection: ConnectionState): QcConnectionPresentation {
  // A virtual session is never a connected USB device, whatever its phase says.
  if (connection.source === "virtual") return { connected: false, busy: false, label: "CONNECT", appearance: "absent" };
  if (connection.phase === "ready" && !connection.demo) return { connected: true, busy: false, label: "USB", appearance: "connected" };
  if (connection.phase === "syncing") return { connected: false, busy: true, label: "SYNC", appearance: "syncing" };
  if (["discovering", "opening", "handshaking"].includes(connection.phase)) return { connected: false, busy: true, label: "WAIT", appearance: connection.phase === "discovering" ? "available" : "connecting" };
  if (["needs-attention", "degraded"].includes(connection.phase)) return { connected: false, busy: false, label: "CONNECT", appearance: "error" };
  return { connected: false, busy: false, label: "CONNECT", appearance: "absent" };
}

/** Map transport state and the stricter host readiness check to one indicator hue. */
export function qcConnectionIndicatorPhase(
  connection: ConnectionState,
  deviceReady = false,
  syncProgress: number | null = null,
  condition: QcDeviceIndicatorCondition = {}
): QcIndicatorPhase {
  // Read before deviceReady: a virtual session is ready for editing, but it is
  // not a Quad Cortex, so it never borrows the live-hardware green.
  if (connection.source === "virtual") return connection.phase === "ready" ? "demo" : "idle";
  // A host failure is the tray's red UI-failure path and outranks transport
  // history. Ordinary absence does not set this flag and therefore stays gray.
  if (condition.error) return "error";
  // The transport phase is the newer observation when a cable disappears or
  // a live session faults. Never let a stale host-ready flag keep either app
  // green after that terminal observation has arrived.
  if (["needs-attention", "degraded"].includes(connection.phase)) return "error";
  if (["disconnected", "discovering"].includes(connection.phase)) return "idle";
  // A quarantine can outlive the cable. Rank absence above it, exactly as the
  // tray does, so an unplugged device is gray rather than stale amber.
  if (condition.actionRequired) return "action-required";
  // syncProgress is ranked before deviceReady, as qcReadyLabel ranks it: the
  // two describe one indicator, and ordering them differently rendered a green
  // dot captioned "SYNCING 40%" on any host that reports both.
  if (syncProgress !== null) return "syncing";
  if (deviceReady) return "ready";
  if (["opening", "handshaking", "syncing", "ready"].includes(connection.phase)) return "syncing";
  // Discovering is still looking: nothing has answered yet, so it stays the
  // gray of an absent device. Blue is earned by a device that responded -
  // an opening, handshaking or syncing session, or a host's sync progress.
  return "idle";
}

export type QcLiveIndicatorLabel = "LIVE" | "SYNC" | "WAIT" | "ACTION" | "ERROR" | "OFFLINE" | "DEMO";

export interface QcLiveIndicatorState {
  label: QcLiveIndicatorLabel;
  phase: QcIndicatorPhase;
  /** A pull is still streaming while every control already works. */
  backgroundSync: boolean;
}

const indicatorLabel: Record<QcIndicatorPhase, QcLiveIndicatorLabel> = {
  ready: "LIVE",
  demo: "DEMO",
  syncing: "SYNC",
  available: "WAIT",
  "action-required": "ACTION",
  error: "ERROR",
  idle: "OFFLINE"
};

/**
 * The prominent device pill: one hue, one word, and whether a pull is still
 * arriving behind a device that already works.
 *
 * `backgroundSync` is deliberately subordinate to readiness rather than a
 * sixth phase. A catalog pull costs ~585 messages over ~11 seconds, so one
 * is in flight for a large share of any session; letting it
 * own the indicator would spell a fully usable QC "SYNC" and read as an
 * outage. It therefore annotates a live device and is dropped entirely
 * whenever the device is not live - before readiness the handshake's own
 * `syncProgress` already owns the indicator, and claiming "LIVE" beside it
 * would be the green-dot-captioned-SYNCING bug in the other direction.
 */
export function qcLiveIndicator(
  connection: ConnectionState,
  deviceReady = false,
  syncProgress: number | null = null,
  backgroundSync = false,
  condition: QcDeviceIndicatorCondition = {}
): QcLiveIndicatorState {
  const phase = qcConnectionIndicatorPhase(connection, deviceReady, syncProgress, condition);
  // A connect still looking for its device is waiting, not offline. It keeps
  // the badge's gray - nothing has answered, so blue is not yet earned - but
  // says WAIT, so the pill never calls an attempt in progress OFFLINE.
  const label = phase === "idle" && connection.phase === "discovering" ? "WAIT" : indicatorLabel[phase];
  return { label, phase, backgroundSync: backgroundSync && phase === "ready" };
}

/**
 * The device-readiness wording both hosts show, in one place.
 *
 * Windows and Android had each grown their own vocabulary for the same states -
 * "QC READY" against "USB", "SYNCING 40%" against "SYNC" - so the two apps
 * described one device in two languages. `deviceReady` and `syncProgress` are
 * optional because only a host that verifies readiness separately supplies
 * them; without them the phase alone decides.
 */
export function qcReadyLabel(
  connection: ConnectionState,
  deviceReady?: boolean,
  syncProgress?: number | null
): string {
  if (syncProgress !== undefined && syncProgress !== null) return `SYNCING ${syncProgress}%`;
  if (connection.source === "virtual") return connection.phase === "ready" ? "DEMO DEVICE" : "DISCONNECTED";
  if (deviceReady) return "QC READY";
  // Ready transport with unverified device state is still checking, but a host
  // that does not verify separately treats a ready phase as ready.
  if (connection.phase === "ready" && !connection.demo) {
    return deviceReady === undefined ? "QC READY" : "CHECKING QC";
  }
  if (connection.phase === "syncing") return "SYNCING";
  if (["needs-attention", "degraded"].includes(connection.phase)) return "QC OFFLINE";
  if (connection.phase === "disconnected") return "DISCONNECTED";
  return connection.phase.replace("-", " ").toUpperCase();
}

export type QcRelayPresentation = {
  label: "REMOTE";
  phase: QcIndicatorPhase;
  state: string;
  detail: string;
  primaryAction: "Checking status" | "Pair device" | "Reconnect" | "Connected" | "Connecting…" | "Waiting for sign-in";
  /** The native action to run. UI wording must never be used as control flow. */
  action: "checking" | "pair" | "reconnect" | "none";
};

/** One stable name and one vocabulary for the outbound remote route. */
export function qcRelayPresentation(status?: PublicRelayStatus, pairingInProgress = false, pairingError?: string): QcRelayPresentation {
  if (pairingError) return { label: "REMOTE", phase: "error", state: "Unavailable", detail: pairingError, primaryAction: "Pair device", action: "pair" };
  if (pairingInProgress) return { label: "REMOTE", phase: "syncing", state: "Waiting for sign-in", detail: "Finish secure sign-in in your browser. QC Remote will finish pairing automatically.", primaryAction: "Waiting for sign-in", action: "none" };
  if (!status) return { label: "REMOTE", phase: "syncing", state: "Checking", detail: "Checking whether remote access is available.", primaryAction: "Checking status", action: "checking" };
  if (!status.paired) return status.needsRepair
    ? { label: "REMOTE", phase: "action-required", state: "Pair again", detail: "Remote access now uses Google sign-in. Pair this device again to continue.", primaryAction: "Pair device", action: "pair" }
    : { label: "REMOTE", phase: "action-required", state: "Not set up", detail: "Pair this device once to use it from a supported AI app away from this host.", primaryAction: "Pair device", action: "pair" };
  if (status.state === "connected") return { label: "REMOTE", phase: "ready", state: "Ready", detail: "This host's secure outbound relay route is connected.", primaryAction: "Connected", action: "none" };
  if (status.state === "connecting" || status.state === "reconnecting") return { label: "REMOTE", phase: "syncing", state: "Connecting", detail: "Establishing the secure outbound connection…", primaryAction: "Connecting…", action: "none" };
  if (status.state === "pairing_required") return { label: "REMOTE", phase: "action-required", state: "Pair again", detail: "Remote authorization is no longer valid. Pair again to restore access.", primaryAction: "Pair device", action: "pair" };
  if (status.state === "invalid_endpoint") return { label: "REMOTE", phase: "action-required", state: "Pair again", detail: "Remote setup is no longer valid. Pair again to restore access.", primaryAction: "Pair device", action: "pair" };
  return { label: "REMOTE", phase: "action-required", state: "Paused", detail: "This device is paired, but the remote route is not connected.", primaryAction: "Reconnect", action: "reconnect" };
}

export type QcRemoteDeviceRow = {
  deviceId: string;
  name: string;
  status: "Connected" | "Paired" | "Expired";
  tone: QcIndicatorPhase;
  email?: string;
  /** The browser the sign-in was completed in. Not the device, and never shown
   * as one; it is how someone tells two sign-ins on one account apart. */
  signedInFrom?: string;
  pairedAt: number;
  /** Which glyph to draw. Inferred from the name the device registered under,
   * because the relay records no platform field; a wrong guess costs an icon,
   * never a capability. */
  platform: "mobile" | "desktop";
  /** The host reading this list. The panel hides this row: the person is
   * standing at this machine and pairing it, so listing it back to them
   * displaces the rows that answer "what else can reach my rig". */
  current: boolean;
};

/** The one account every paired device shares.
 *
 * Pairing binds a device to a Google account, and every device on that account
 * reaches the same Durable Object. The account is therefore the unit a person
 * reasons about, and the only one worth putting a single control against; the
 * devices under it are a record of what has joined.
 *
 * `pairedAt` is the earliest pairing on the account - when this account first
 * gained access - not the most recent device to join. */
export type QcRemoteAccount = {
  email?: string;
  pairedAt: number;
  deviceCount: number;
};

export function qcRemoteAccount(rows: readonly QcRemoteDeviceRow[]): QcRemoteAccount | undefined {
  if (rows.length === 0) return undefined;
  const email = rows.find((row) => row.email)?.email;
  const pairedAt = rows.reduce((earliest, row) => Math.min(earliest, row.pairedAt), Number.POSITIVE_INFINITY);
  return {
    ...(email ? { email } : {}),
    pairedAt: Number.isFinite(pairedAt) ? pairedAt : 0,
    deviceCount: rows.length
  };
}

const MOBILE_NAME_HINTS = ["android", "phone", "pixel", "galaxy", "samsung", "oneplus", "xiaomi", "moto"];

function devicePlatform(name: string): "mobile" | "desktop" {
  const lower = name.toLowerCase();
  return MOBILE_NAME_HINTS.some((hint) => lower.includes(hint)) ? "mobile" : "desktop";
}

/** The devices the REMOTE panel lists beneath its pairing code.
 *
 * `roster` is what the relay reports for the whole account, and is
 * authoritative whenever it answers: it is the only thing that knows about the
 * phone standing next to the computer. A relay older than the roster route,
 * one that cannot be reached, or a host that has no way to ask, all yield
 * nothing - and this host still knows the pairing it performed itself, so the
 * list falls back to that single row rather than reporting an account with
 * devices on it as empty.
 *
 * An unpaired host has no rows at all. That is why the pairing code lives
 * above this list and not inside it: with no pairing there is still something
 * to show, and with one there is still something to scan. */
export function qcRemoteDeviceRows(status?: PublicRelayStatus, roster: readonly PublicRelayDevice[] = []): QcRemoteDeviceRow[] {
  const connectedHere = status?.state === "connected";
  const devices: PublicRelayDevice[] = roster.length > 0
    ? [...roster]
    : status?.paired && status.pairing
      ? [{
          deviceId: status.deviceId ?? "",
          // The relay stores a name for this host; locally all this host kept
          // was the browser the sign-in happened in, which is not the device.
          name: "This computer",
          pairedAt: status.pairing.pairedAt,
          expiresAt: 0,
          email: status.pairing.email,
          signedInFrom: status.pairing.device,
          connected: connectedHere,
          current: true
        }]
      : [];
  return devices.map((device) => {
    const current = device.current === true;
    // This host reports its own connection from its own relay state. The
    // roster's `connected` is a socket the relay can see, which for the host
    // asking is the same fact arriving more slowly.
    const connected = current ? connectedHere : device.connected === true;
    const status_ = device.expired ? "Expired" as const : connected ? "Connected" as const : "Paired" as const;
    return {
      deviceId: device.deviceId,
      name: device.name,
      status: status_,
      tone: device.expired ? "action-required" as const : connected ? "ready" as const : "available" as const,
      ...(device.email ? { email: device.email } : {}),
      ...(device.signedInFrom ? { signedInFrom: device.signedInFrom } : {}),
      pairedAt: device.pairedAt,
      platform: devicePlatform(device.name),
      current
    };
  });
}

/** Shared app-level connection state; native adapters only report transitions. */
export function useQcConnectionWorkflow(initial: ConnectionState) {
  const [connection, setConnection] = useState(initial);
  // Connection attempts outlive renders, but must never outlive a replacement
  // attempt, detach, or component mount. Keep this separate from preset edits.
  const attempts = useMemo(() => {
    let revision = 0;
    const capture = () => {
      const origin = revision;
      return { current: () => origin === revision };
    };
    return {
      capture,
      begin: () => { revision += 1; return capture(); },
      invalidate: () => { revision += 1; }
    };
  }, []);
  useEffect(() => () => attempts.invalidate(), [attempts]);
  const transition = useCallback((status: QcConnectionTransition, detail = transitionDetail[status]) => {
    setConnection({ phase: transitionPhase[status], detail, demo: status !== "connected" && status !== "syncing", source: "device" });
  }, []);
  const presentation = useMemo(() => qcConnectionPresentation(connection), [connection]);
  return { connection, setConnection: setConnection as Dispatch<SetStateAction<ConnectionState>>, transition, attempts, ...presentation };
}
