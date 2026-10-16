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

/** The primary pushed recovery path respects the same deliberate stop as inventory recovery. */
export function qcShouldFollowDevicePhase(event: { phase: string; deviceId?: string }, selectedDeviceId: string | undefined, userDisconnected: boolean): boolean {
  return !userDisconnected
    && !(selectedDeviceId && event.deviceId && event.deviceId !== selectedDeviceId)
    && brokerPhaseDetectsDevice(event.phase);
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

/**
 * Every `broker.devices` row as the window may still show it once the poll
 * has failed. A failed poll is not "the last answer still holds": the broker
 * may have quit, released the device or lost it, so no row may keep a phase
 * that renders green. Ids and names stay, so the tabs do not jump while the
 * broker is away. Rows already cleared are returned as they are, so a broker
 * that stays away does not re-render the window on every poll.
 */
export function qcInventoryWhenUnreachable<Row extends QcInventoryDevice>(rows: Row[]): Row[] {
  if (rows.every((row) => row.connected === false && row.phase === "disconnected")) return rows;
  return rows.map((row) => ({ ...row, connected: false, phase: "disconnected" }));
}

/**
 * What the window knows of the broker, from the inventory poll alone. It is
 * never a cached snapshot: a poll that fails makes the broker unreachable and
 * its device not ready at once (`deviceReady`, what the USB tabs draw).
 *
 * What the broker last *answered* is kept apart (`answeredReady`): a failed
 * poll says nothing about the device, so it never changes that value and can
 * never make the next answer look like a new edge.
 *
 * `generation` moves only when an answered poll names a different broker
 * instance than the last one did (`brokerInstanceId`, added by the host). An
 * answer without an instance id (a detection push, an older host) never
 * moves it. `readyEdges` counts answers that read the selected device ready
 * after an answer that read it not ready, the first ready answer, and a
 * ready answer from a new generation: a new device session.
 */
export interface QcBrokerTruth {
  /** null until the first observation; false after a poll that failed. */
  reachable: boolean | null;
  /** The selected (or only) device is connected and ready right now; false while unreachable. */
  deviceReady: boolean;
  /** What the last answered observation read; null before any answer. Failed polls leave it alone. */
  answeredReady: boolean | null;
  instanceId: string | null;
  generation: number;
  readyEdges: number;
}

export const QC_BROKER_TRUTH_UNKNOWN: QcBrokerTruth = { reachable: null, deviceReady: false, answeredReady: null, instanceId: null, generation: 0, readyEdges: 0 };

/** One settled inventory poll (or detection push): what the broker answered, or that the poll failed. */
export type QcBrokerObservation =
  | { kind: "answered"; inventoryReady: boolean; instanceId?: string | null }
  | { kind: "unreachable" };

export function qcObserveBroker(truth: QcBrokerTruth, observation: QcBrokerObservation): QcBrokerTruth {
  if (observation.kind === "unreachable") return { ...truth, reachable: false, deviceReady: false };
  const instanceId = observation.instanceId ?? null;
  const newInstance = instanceId !== null && truth.instanceId !== null && instanceId !== truth.instanceId;
  const generation = newInstance ? truth.generation + 1 : truth.generation;
  const edge = observation.inventoryReady && (newInstance || truth.answeredReady !== true);
  return {
    reachable: true,
    deviceReady: observation.inventoryReady,
    answeredReady: observation.inventoryReady,
    instanceId: instanceId ?? truth.instanceId,
    generation,
    readyEdges: edge ? truth.readyEdges + 1 : truth.readyEdges
  };
}

/** Whether two observations of broker truth are the same, so nothing needs drawing. */
export function qcSameBrokerTruth(left: QcBrokerTruth, right: QcBrokerTruth): boolean {
  return left.reachable === right.reachable && left.deviceReady === right.deviceReady && left.answeredReady === right.answeredReady
    && left.instanceId === right.instanceId && left.generation === right.generation && left.readyEdges === right.readyEdges;
}

/**
 * The user's Disconnect, as a latch over broker truth (owner decision,
 * 2026-10-16). It is lifted only by:
 *
 * - the user's own Connect (App drops the latch), or
 * - a new device session the broker itself reports: an *answered* not-ready
 *   followed by an *answered* ready, both from the same broker generation
 *   the Disconnect was made against.
 *
 * A new broker generation alone never lifts it - not a restart, a crash and
 * respawn, nor a broker this window's own host started - and after one only
 * the user's Connect does. A failed poll counts as nothing at all, so a
 * Disconnect that failed (the device stays open and ready) or a poll that
 * answered before the close landed never lifts it. None of this touches the
 * window without a Disconnect: a broker quit, crash or restart and a cable
 * unplug or replug always return it to Live (qcFollowBroker).
 *
 * A lifted latch is kept (`lifted`) until the window is live again
 * (qcFollowBroker drops it then): the window still sits in the
 * "disconnected" state that Disconnect left, which a follow otherwise never
 * leaves (qcShouldConnectOnDetection), and the lifted latch is what lets it
 * mirror the broker's new session. App also drops it on Stop demo, whose own
 * "disconnected" is never followed.
 */
export interface QcUserDisconnect { generation: number; sawNotReady: boolean; lifted: boolean }

export function qcLatchUserDisconnect(truth: QcBrokerTruth): QcUserDisconnect {
  return { generation: truth.generation, sawNotReady: false, lifted: false };
}

/** The latch after one observation (`truth` as qcObserveBroker left it). */
export function qcUserDisconnectAfter(latch: QcUserDisconnect | null, truth: QcBrokerTruth): QcUserDisconnect | null {
  if (!latch || latch.lifted) return latch;
  // A failed poll, or another broker generation, is no evidence either way.
  if (truth.reachable !== true || truth.generation !== latch.generation) return latch;
  if (truth.answeredReady === false) return latch.sawNotReady ? latch : { ...latch, sawNotReady: true };
  if (truth.answeredReady === true && latch.sawNotReady) return { ...latch, lifted: true };
  return latch;
}

/** Whether the user's Disconnect still holds every automatic follow. */
export function qcUserDisconnectHolds(latch: QcUserDisconnect | null): boolean {
  return latch !== null && !latch.lifted;
}

/**
 * Whether the window may claim a Quad Cortex "connected in the background".
 * Only while the demo is the active view (`demoActive`: outside it there is
 * no background, so an idle USB panel never offers Disconnect for it) and the
 * broker says so now: its inventory reads the device ready and this window's
 * own status reads it synchronized. A broker that quit, released the device
 * or lost it turns the USB tab gray at once.
 */
export function qcBackgroundDeviceLive(held: boolean, demoActive: boolean, truth: QcBrokerTruth, runtime?: RuntimeStatus): boolean {
  return held && demoActive && truth.deviceReady && runtimeHasSynchronizedQc(runtime);
}

/**
 * What Stop demo does. With a device live behind the demo it resumes it;
 * otherwise it goes idle, and a stale "held in the background" flag is
 * cleared with it, so the idle USB panel reads Connect and the USB tab
 * connects normally after a replug.
 */
export function qcStopDemoOutcome(backgroundLive: boolean): { resume: boolean; clearHeld: boolean } {
  return backgroundLive ? { resume: true, clearHeld: false } : { resume: false, clearHeld: true };
}

/** The automatic follow's carried state. */
export interface QcFollowState {
  /** Consecutive automatic connects on the current edge/generation not followed by a session that stayed live. */
  attempts: number;
  /** Earliest monotonic time the next automatic connect may start, or null for now. */
  nextAt: number | null;
  /** An automatic connect fired and no decision has yet seen it end. */
  inFlight: boolean;
  /** Whether that connect was started from a demo the user was working in. */
  fromDemo: boolean;
  /**
   * Automatic connects from the demo, on the current edge/generation, that
   * ended without the window reaching live. Any live session, a new edge or
   * generation, or the user's Connect clears it; it is what caps demo
   * takeovers (QC_FOLLOW_DEMO_ATTEMPTS), apart from the backoff's `attempts`.
   */
  demoFailures: number;
  /** When the polls first saw the current live session, or null. */
  liveSince: number | null;
  /** The broker generation and ready edge the last decision saw. */
  seenGeneration: number;
  seenReadyEdges: number;
}

export const QC_FOLLOW_INITIAL: QcFollowState = { attempts: 0, nextAt: null, inFlight: false, fromDemo: false, demoFailures: 0, liveSince: null, seenGeneration: 0, seenReadyEdges: 0 };

/** The first retry's delay after an automatic connect that did not stay live. */
export const QC_FOLLOW_RETRY_MIN_MS = 4_000;
/** The backoff's ceiling: a divergence that never heals is retried this often from an idle window. */
export const QC_FOLLOW_RETRY_MAX_MS = 60_000;
/**
 * How long the polls must see a session stay live before the backoff starts
 * over. A health check that fails sooner than this after every connect keeps
 * growing the backoff instead of reconnecting every few seconds.
 */
export const QC_FOLLOW_STABLE_LIVE_MS = 60_000;
/**
 * Consecutive automatic connects from the demo, on one ready edge and
 * generation, that never reached live, after which a demo the user is
 * working in is no longer taken over. A connect that reached live, even
 * briefly, does not count (it clears the count). Each
 * attempt replaces the demo on screen with the connecting placeholder, so a
 * device the broker keeps ready but that never synchronizes would otherwise
 * interrupt the demo every minute. The window stays on the demo and says so
 * (`demoGaveUp`); a new edge or generation, or the user's Connect, starts
 * over. An idle or failed window keeps retrying at the ceiling.
 */
export const QC_FOLLOW_DEMO_ATTEMPTS = 3;

/** The delay after the `attempts`-th consecutive automatic connect. */
export function qcFollowRetryDelay(attempts: number): number {
  return Math.min(QC_FOLLOW_RETRY_MAX_MS, QC_FOLLOW_RETRY_MIN_MS * 2 ** Math.max(0, attempts - 1));
}

/**
 * One decision to follow the broker. The window mirrors it: when the broker
 * reads the selected device ready and the window is not live, the window
 * connects, unless one of the deliberate overrides holds:
 *
 * - the user's Disconnect (`userDisconnect`, see QcUserDisconnect); once
 *   lifted, the "disconnected" state that Disconnect left is followed;
 * - the user's explicit Demo choice (`demoCanYield` false, through
 *   `qcShouldConnectOnDetection`, which also leaves a stopped demo and an
 *   attempt under way alone);
 * - a tray release or a recorded Quit. Neither is visible here and neither
 *   needs to be: the host refuses `broker.devices` for the released
 *   generation, so the poll fails and nothing reads ready until a newer
 *   broker the host has attached to answers.
 *
 * A connect already reaching the device (`connectHeld`, from
 * qcAutomaticConnectHeld) neither fires nor counts, so no second reconnect is
 * ever queued behind one, a reconnect the watchdog gave up on included.
 *
 * Retries. An answered new ready edge or a new broker generation starts over
 * at once (attempts 0, no wait); so does a session the polls saw stay live
 * for QC_FOLLOW_STABLE_LIVE_MS. A failed poll never does. Otherwise each
 * attempt waits qcFollowRetryDelay (4 s, 8 s, 16 s, 32 s, then 60 s),
 * measured from the first decision that sees the previous attempt over, not
 * from its start, so a long handshake does not eat the wait. From a demo,
 * attempts stop after QC_FOLLOW_DEMO_ATTEMPTS (`demoGaveUp`); from an idle or
 * failed window they continue at the ceiling for as long as the broker stays
 * ready. `now` is monotonic.
 *
 * `userDisconnect` comes back dropped once the window is live with a lifted
 * latch.
 */
export function qcFollowBroker(state: {
  follow: QcFollowState;
  truth: QcBrokerTruth;
  now: number;
  userDisconnect: QcUserDisconnect | null;
  connectHeld: boolean;
  connection: ConnectionState;
  demoCanYield: boolean;
}): { connect: boolean; follow: QcFollowState; demoGaveUp: boolean; userDisconnect: QcUserDisconnect | null } {
  const { truth, now, connection } = state;
  const live = connection.source === "device" && connection.phase === "ready";
  const userDisconnect = live && state.userDisconnect?.lifted ? null : state.userDisconnect;
  const liveSince = live ? (state.follow.liveSince ?? now) : null;
  const fresh = truth.generation !== state.follow.seenGeneration || truth.readyEdges !== state.follow.seenReadyEdges;
  const stable = liveSince !== null && now - liveSince >= QC_FOLLOW_STABLE_LIVE_MS;
  const restart = fresh || stable;
  const attempts = restart ? 0 : state.follow.attempts;
  const ended = state.follow.inFlight && !state.connectHeld;
  const nextAt = restart ? null : ended ? now + qcFollowRetryDelay(attempts) : state.follow.nextAt;
  // A takeover from the demo counts as failed when it ends; reaching live,
  // which App shows only after it ended, clears the count again.
  const demoFailures = live || fresh ? 0 : state.follow.demoFailures + (ended && state.follow.fromDemo ? 1 : 0);
  const follow: QcFollowState = {
    attempts,
    nextAt,
    inFlight: state.follow.inFlight && !ended,
    fromDemo: state.follow.fromDemo,
    demoFailures,
    liveSince,
    seenGeneration: truth.generation,
    seenReadyEdges: truth.readyEdges
  };
  const demoGaveUp = connection.source === "virtual" && demoFailures >= QC_FOLLOW_DEMO_ATTEMPTS;
  const hold = { connect: false, follow, demoGaveUp, userDisconnect };
  if (!truth.deviceReady || live || qcUserDisconnectHolds(userDisconnect) || state.connectHeld || follow.inFlight) return hold;
  const leftByLiftedDisconnect = userDisconnect?.lifted === true && connection.source === "device" && connection.phase === "disconnected";
  if (!leftByLiftedDisconnect && !qcShouldConnectOnDetection(connection, state.demoCanYield)) return hold;
  if (demoGaveUp || (follow.nextAt !== null && now < follow.nextAt)) return hold;
  return { connect: true, follow: { ...follow, attempts: attempts + 1, nextAt: null, inFlight: true, fromDemo: connection.source === "virtual" }, demoGaveUp: false, userDisconnect };
}

/**
 * A session the window had and lost, or a connect it gave up on: the live
 * measurement starts over, so a drop another path heals before any poll saw
 * it is never counted as stable live time. `userAction` is for a loss the
 * user's own choice ran into (a failed USB-tab resume): like their Connect,
 * it starts the attempts over. It never lifts the user's Disconnect.
 */
export function qcRecordLiveSessionLoss(follow: QcFollowState, userAction = false): QcFollowState {
  return userAction ? { ...follow, liveSince: null, attempts: 0, nextAt: null, demoFailures: 0 } : { ...follow, liveSince: null };
}

/** What the user's own Connect does to the follow: every attempt count and wait starts over. */
export function qcFollowAfterUserConnect(follow: QcFollowState): QcFollowState {
  return { ...follow, attempts: 0, nextAt: null, demoFailures: 0 };
}

/**
 * Whether the answer to the user's Disconnect says the session really closed.
 * A broker that left a shared session open for another client (its
 * "the shared session was left open" answer, or any phase but
 * "disconnected") closed nothing (qcDisconnectOutcome).
 */
export function qcDisconnectClosedSession(result: { phase: string; detail?: string }): boolean {
  return result.phase === "disconnected" && !/session (?:was left|stays) open|still attached/i.test(result.detail ?? "");
}

/** Shown when a broker leaves the session open without saying why. */
export const QC_SESSION_LEFT_OPEN_NOTICE = "Another client is still attached; the session was left open";

/**
 * What the window shows and holds after the user's Disconnect answered.
 *
 * A closed session keeps the latch and the broker's own answer.
 *
 * A session the broker left open (another client is still attached) closed
 * nothing, and the window must not bounce through a fresh `device.reconnect`
 * to find that out (owner decision, 2026-10-16). So the latch stays *held*
 * here - no follow fires - while App restores Live directly, the way it
 * resumes a background session: re-verify the broker's status, re-read the
 * snapshot, no reconnect. App drops the latch once that restore succeeds and
 * keeps it held if it fails. Either way the broker's reason stays on screen
 * (`notice`) and is logged as a warning. The window is never shown "ready"
 * from the raw answer, which comes without a synchronization.
 */
export function qcDisconnectOutcome(latch: QcUserDisconnect | null, result: ConnectionState): { closed: boolean; connection: ConnectionState; latch: QcUserDisconnect | null; notice: string } {
  if (qcDisconnectClosedSession(result)) return { closed: true, connection: result, latch, notice: result.detail };
  const notice = result.detail || QC_SESSION_LEFT_OPEN_NOTICE;
  return { closed: false, connection: { phase: "disconnected", detail: notice, demo: true, source: "device" }, latch, notice };
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
 * Quit stops (see qcFollowBroker).
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
 * reconnecting to the follow above (qcFollowBroker), which holds until that reconnect
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
