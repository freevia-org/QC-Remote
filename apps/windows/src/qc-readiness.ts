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

/** The fields of one `broker.devices` row this decision reads. */
export interface QcInventoryDevice { id: string; connected?: boolean; phase?: string }

/**
 * Whether the broker's device inventory shows the Quad Cortex this window
 * would connect to already open and ready: the selected one, or the only one
 * while nothing is selected. With several and no selection there is no device
 * this window's connect would be known to reach, so none counts.
 */
export function qcInventoryDeviceReady(devices: readonly QcInventoryDevice[], selectedId?: string | null): boolean {
  const device = selectedId ? devices.find((row) => row.id === selectedId) : devices.length === 1 ? devices[0] : undefined;
  return device?.connected === true && device.phase === "ready";
}

export interface QcReadyInventoryStep {
  /** Start the same connect a detected device starts. */
  connect: boolean;
  /** Carried to the next poll: this ready edge has been acted on. */
  followed: boolean;
  /** Carried: re-follows spent on lost live sessions since the last give-back. */
  refollows: number;
  /** Carried: a live session was lost and no connect from here answered it. */
  liveSessionLost: boolean;
  /** Carried: when a poll first saw the current live session, or null. */
  liveSince: number | null;
}

/**
 * Re-follows the inventory poll may spend on lost live sessions before one is
 * given back. One: a live-health check that keeps failing must not turn into
 * connect, sync, fail, connect again.
 */
export const QC_READY_INVENTORY_REFOLLOWS = 1;

/**
 * How long the polls must see a session stay live before a spent re-follow is
 * given back. A ready edge can last all day beside a resident broker, so a
 * budget that only a new edge restored would leave the second loss of the day
 * unfollowed; one that came back sooner would let a flapping health check
 * reconnect every few seconds.
 */
export const QC_REFOLLOW_RESTORE_AFTER_LIVE_MS = 60_000;

/**
 * One inventory poll's decision to follow a device the broker already has
 * ready. It began as the safety net beside the `device.phase` push: a broker
 * that never pushes it (a tray resident older than this window) left the
 * window on the demo until someone clicked Reconnect, although the tray showed
 * the device connected. It is now the one place automatic follows are decided
 * (see "Who decides" below).
 *
 * One connect per transition of the inventory into ready: whatever it ends
 * in, a refusal included, the next one waits for the device to leave ready
 * first. A connect already under way neither fires nor spends the edge, so a
 * ready device it misses is followed once it ends. A live session spends the
 * edge without connecting. A Demo choice never connects
 * (`qcShouldConnectOnDetection`), and neither does anything after the user's
 * Disconnect until their own Connect: `userDisconnected` carries that,
 * because a Disconnect that failed leaves "needs-attention", which a
 * detection may otherwise follow, with the device still open.
 *
 * A live session the window then loses by its own live-health check
 * (`liveSessionLost`, set by beginLiveRecovery) is followed again on the same
 * edge, since a broker that stays ready makes no new edge. That re-follow is
 * capped by `QC_READY_INVENTORY_REFOLLOWS` and given back only by the
 * inventory leaving ready, by the user's own Connect (in App), or by the polls
 * seeing a session stay live for `QC_REFOLLOW_RESTORE_AFTER_LIVE_MS`. Any
 * poll that finds the window not live restarts that measurement, and so does
 * every loss App records (it clears `liveSince` beside `liveSessionLost`). A
 * re-follow only ever fires from such a poll, so unless the user's own
 * Connect gives the budget back sooner, two re-follows on one edge are at
 * least that long apart on the caller's clock (`now`, which must be
 * monotonic). An automatic connect is an unforced `device.reconnect`, which a
 * ready broker answers without touching its session, so it cannot make the
 * edge that re-arms it. The bound, for connects this function starts: per
 * ready edge one edge follow plus one re-follow, and after that at most one
 * re-follow per `QC_REFOLLOW_RESTORE_AFTER_LIVE_MS` of observed live time; a
 * health check that fails sooner than that after every connect gets no more
 * than those two.
 *
 * Who decides. The inventory poll calls this with what `broker.devices`
 * actually answered. The demo fallback's retry never calls it: it only records
 * a pending follow (`qcRecordLiveSessionLoss`), which the next poll that reads
 * the device ready answers within the budget above. The `device.phase`
 * listener calls it (as `inventoryReady: true`) only while no poll has ever
 * answered in this window (`qcPhaseListenerMayFollow`), for a broker the poll
 * cannot read; once one has, a push follows nothing by itself and the poll
 * decides. So a push on an edge already spent is not followed by the push:
 * the next poll follows it only once a poll has seen the inventory leave ready
 * (re-arming the edge) and a later one sees it ready again, or, on the same
 * edge, when a recorded loss is pending and the budget allows a re-follow.
 *
 * A tray release and a recorded Quit must never reach a decision that
 * connects: the host lets `device.reconnect` through a tray release and
 * clears the release on it, and a Quit record is cleared only by the user. The
 * poll cannot reach this after either, because `broker.devices` itself is
 * refused after a release or Quit, the poll throws, and no inventory is ever
 * read as ready. The demo retry does not decide here. The phase listener can
 * reach this only for a broker no poll has answered; that one residual path
 * relies on the host retiring a released broker connection (its pushes stop
 * with it) and on a Quit stopping the broker. The user's own Disconnect is
 * the only one of the three this function can see, so it has a latch
 * (`userDisconnected`). A loss recorded before a release stays pending
 * through it and may be followed, within the same budget, once the polls
 * resume.
 */
export function qcFollowReadyInventory(state: {
  inventoryReady: boolean;
  followed: boolean;
  refollows: number;
  liveSessionLost: boolean;
  liveSince: number | null;
  now: number;
  userDisconnected: boolean;
  connectInFlight: boolean;
  connection: ConnectionState;
  demoCanYield: boolean;
}): QcReadyInventoryStep {
  const { connection, now } = state;
  const live = connection.source === "device" && connection.phase === "ready";
  // Continuity is only what the polls saw: any poll that finds the window not
  // live, held or not, starts the measurement over.
  const liveSince = live ? (state.liveSince ?? now) : null;
  const refollows = liveSince !== null && now - liveSince >= QC_REFOLLOW_RESTORE_AFTER_LIVE_MS ? 0 : state.refollows;
  const held = { connect: false, followed: state.followed, refollows, liveSessionLost: state.liveSessionLost, liveSince };
  if (!state.inventoryReady) return { connect: false, followed: false, refollows: 0, liveSessionLost: false, liveSince: null };
  if (state.userDisconnected || state.connectInFlight) return held;
  // Live again, whoever connected it: the loss is answered and the edge spent.
  if (live) return { ...held, followed: true, liveSessionLost: false };
  if (!qcShouldConnectOnDetection(connection, state.demoCanYield)) return held;
  if (!state.followed) return { ...held, connect: true, followed: true, liveSessionLost: false };
  if (state.liveSessionLost && refollows < QC_READY_INVENTORY_REFOLLOWS) {
    return { ...held, connect: true, followed: true, refollows: refollows + 1, liveSessionLost: false };
  }
  return held;
}

/** The part of qcFollowReadyInventory's state the window carries between calls. */
export type QcReadyInventoryCarried = Omit<QcReadyInventoryStep, "connect">;

/**
 * Record a session the window had, or was handed back to, and lost while the
 * broker may still hold it ready, so the bounded follow above may answer it
 * once. Every such loss restarts the live measurement. `userAction` is for a
 * loss the user's own choice ran into (selecting the USB tab of a device held
 * in the background, whose resume then failed): like their Connect, it gives
 * the re-follow budget back. It never clears `userDisconnected`; only the
 * user's Connect does that.
 */
export function qcRecordLiveSessionLoss(carried: QcReadyInventoryCarried, userAction = false): QcReadyInventoryCarried {
  return { ...carried, liveSessionLost: true, liveSince: null, refollows: userAction ? 0 : carried.refollows };
}

/**
 * How long one connect() may wait for the broker's `device.reconnect` answer
 * before the window gives up on it. The broker's own handshake gives up after
 * 30 s and the boot directory read takes about 11 s, so a connect that is
 * still waiting after this long is not a slow device; the host's own gateway
 * timeout is five minutes, during which the window would otherwise stay gray.
 */
export const QC_CONNECT_STALL_MS = 120_000;

/**
 * How long the window may show the transitional "discovering" state with
 * nothing that owns it (no connect waiting, no command, no demo opening)
 * before it repeats the fallback that state announced. returnToDemoDevice
 * shows it and then hands over to startDemoDevice, which refuses while a
 * command is pending; the refusal left the window gray for good.
 */
export const QC_ORPHANED_DISCOVERING_MS = 6_000;

/**
 * Whether an automatic follow must hold because a connect may already be
 * reaching the device: a connect() waiting for its answer, a command or a demo
 * open, or a `device.reconnect`/`device.resetSession` this window gave up on
 * (the watchdog below) that has not settled yet. The host serializes gateway
 * requests, so a second reconnect issued then would only wait behind the
 * first and run the moment it ends; holding until it settles keeps every
 * automatic follow from adding one. It does not hold the user's own Connect or
 * Reset Session, which may still queue behind an abandoned reconnect.
 */
export function qcAutomaticConnectHeld(state: {
  awaitingDetection: boolean;
  commandPending: boolean;
  demoPending: boolean;
  reconnectsOutstanding: number;
}): boolean {
  return state.awaitingDetection || state.commandPending || state.demoPending || state.reconnectsOutstanding > 0;
}

/**
 * Whether a `device.phase` push may decide a follow by itself. Only while no
 * inventory poll has ever answered in this window: once one has, the poll
 * decides from what the broker actually reports, which a tray release or a
 * Quit stops (see qcFollowReadyInventory).
 */
export function qcPhaseListenerMayFollow(inventoryPollAnswered: boolean): boolean {
  return !inventoryPollAnswered;
}

/** What a stalled connection is: a connect() still waiting, or nothing at all. */
export type QcConnectStallKind = "connect" | "orphaned";
export interface QcConnectStall {
  kind: QcConnectStallKind;
  since: number;
  /** The waiting connect's attempt, or null for an orphaned state. */
  attempt: number | null;
}
/**
 * What the caller does when the watchdog gives up. A stalled connect that
 * started from the demo returns to it, as that connect's own failure would;
 * any other stalled connect ends in "needs-attention" with the stall's detail,
 * as its own failure would. An orphaned "discovering" repeats the demo
 * fallback it announced.
 */
export type QcConnectStallAction = "return-to-demo" | "needs-attention" | "retry-fallback";

/**
 * One watchdog tick over the window's connection. It never connects: on
 * `abandon` the caller supersedes the connect still waiting (its late answer
 * is then ignored), records the reconnect it left outstanding, and leaves
 * reconnecting to the bounded follow above, which holds until that reconnect
 * settles (`qcAutomaticConnectHeld`). A connect the user is waiting on counts
 * the same as an automatic one.
 *
 * `connect` is the connect() waiting for its answer, and only while it is
 * still the current attempt: one the user superseded (Stop demo, Disconnect,
 * Demo choice, another Connect) is not this watchdog's to end, so their choice
 * is never undone here. The clock (`now`, monotonic) restarts whenever the
 * kind of stall or the waiting attempt changes, so a Reset Session or menu
 * Reconnect started inside an old stall gets its own full interval, and so
 * does a fallback that follows a long connect; and it restarts on every
 * abandon, so each one is at least an interval apart.
 */
export function qcWatchConnectStall(state: {
  connection: ConnectionState;
  connect: { attempt: number; fromDemo: boolean } | null;
  /** A device command or a demo open is running and owns the next step. */
  otherWorkPending: boolean;
  stall: QcConnectStall | null;
  now: number;
}): { stall: QcConnectStall | null; abandon: QcConnectStallAction | null } {
  const { connection, connect, now } = state;
  const kind: QcConnectStallKind | null = connect
    ? "connect"
    : connection.source === "device" && connection.phase === "discovering" && !state.otherWorkPending
      ? "orphaned"
      : null;
  if (kind === null) return { stall: null, abandon: null };
  const attempt = connect ? connect.attempt : null;
  const since = state.stall?.kind === kind && state.stall.attempt === attempt ? state.stall.since : now;
  const limit = kind === "connect" ? QC_CONNECT_STALL_MS : QC_ORPHANED_DISCOVERING_MS;
  if (now - since < limit) return { stall: { kind, since, attempt }, abandon: null };
  const abandon: QcConnectStallAction = !connect ? "retry-fallback" : connect.fromDemo ? "return-to-demo" : "needs-attention";
  return { stall: null, abandon };
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
