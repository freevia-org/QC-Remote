import type { PresetSnapshot } from "@qc-remote/client";
import { applyFootswitchPreview } from "./footswitch.ts";
import { samePresetAddress, type PresetAddress } from "./preset-address.ts";
import {
  applyQcStateUpdate,
  bypassAddress,
  movePresetInSnapshot,
  reconcilePresetSnapshot,
  reconcilePendingBypass,
  selectModeSlotInSnapshot,
  selectSceneInSnapshot,
  setBlockBypassInSnapshot,
  setTempoInSnapshot,
  type PendingBypassIntents,
  type QcStateUpdate
} from "./state.ts";

type ScalarKey = "scene" | "position" | "mode" | "tempo";

type ScalarValueByKey = {
  scene: number;
  position: number;
  mode: PresetSnapshot["mode"];
  tempo: number;
};

/** One scalar field together with the UI state that is restored with it. */
type ScalarState = {
  key: ScalarKey;
  previous: number | PresetSnapshot["mode"];
  previousModeSlots?: PresetSnapshot["modeSlots"];
  previousFootswitchModes?: PresetSnapshot["footswitchModes"];
  previousTempoLedEnabled?: boolean;
};

/** The newest value the device is known to hold, and when it was observed. */
type ScalarBase = ScalarState & { observedAt: number };

type ScalarIntent<K extends ScalarKey = ScalarKey> = {
  id: number;
  key: K;
  target: ScalarValueByKey[K];
  issuedAt: number;
  /**
   * Superseding intents inherit this base instead of snapshotting the older
   * optimistic value, so rejected rapid intents roll back to what the device
   * holds rather than to an intermediate value nobody confirmed.
   */
  base: ScalarBase;
};

type ScalarRollback = ScalarState & {
  /** This command's own result; it becomes the base if it succeeds after being superseded. */
  applied: ScalarState;
};

type BypassRollback = {
  address: string;
  row: number;
  column: number;
  previous: boolean | undefined;
  target: boolean;
};

type BypassBase = { bypassed: boolean | undefined; observedAt: number };

type FootswitchBase = {
  states: PresetSnapshot["footswitchStates"];
  observedAt: number;
  commandIds: Set<number>;
};

/** Older than every real observation: a base taken from the pre-intent snapshot. */
const PRE_INTENT = Number.NEGATIVE_INFINITY;

export const QC_OPTIMISTIC_COMMAND_GRACE_MS = 3_000;

/** Opaque handle used to settle or safely roll back one optimistic command. */
export interface QcCommandToken {
  readonly id: number;
  readonly issuedAt: number;
  readonly scalarRollbacks: readonly ScalarRollback[];
  readonly bypassRollbacks: readonly BypassRollback[];
  readonly previousFootswitchStates?: PresetSnapshot["footswitchStates"];
  readonly appliedFootswitchStates?: PresetSnapshot["footswitchStates"];
}

export interface QcOptimisticCommand {
  snapshot: PresetSnapshot;
  token: QcCommandToken;
}

/**
 * Platform-neutral lifecycle for realtime QC commands.
 *
 * Native transports only send commands and timestamp device observations. This
 * coordinator owns instant UI previews, stale-echo suppression, acknowledgement,
 * and rollback without allowing an older failure to undo a newer command.
 * Rollback always targets the newest authoritative value of a field: the
 * pre-intent snapshot, then any device observation or superseded command
 * success that arrives while a newer intent for that field is pending.
 */
export class QcCommandCoordinator {
  private readonly bypassIntents: PendingBypassIntents = new Map();
  private readonly bypassBases = new Map<string, BypassBase>();
  private readonly scalarIntents = new Map<ScalarKey, ScalarIntent>();
  private footswitchBase?: FootswitchBase;
  private nextId = 1;
  private intentPreset?: PresetAddress;

  reset(): void {
    this.bypassIntents.clear();
    this.bypassBases.clear();
    this.scalarIntents.clear();
    this.footswitchBase = undefined;
    this.intentPreset = undefined;
  }

  private observePreset(address: PresetAddress): void {
    if (this.intentPreset && !samePresetAddress(this.intentPreset, address)) this.reset();
    this.intentPreset = { setlistKey: address.setlistKey, presetPosition: address.presetPosition };
  }

  get hasPendingCommands(): boolean {
    return this.bypassIntents.size > 0 || this.scalarIntents.size > 0;
  }

  /**
   * Time until the oldest optimistic command needs an authoritative refresh.
   * Matching pushed state normally clears intents first; this deadline keeps a
   * lost acknowledgement or suppressed physical override from sticking forever.
   */
  pendingReconciliationDelay(now = Date.now(), graceMs = QC_OPTIMISTIC_COMMAND_GRACE_MS): number | undefined {
    const issuedAt = [
      ...Array.from(this.bypassIntents.values(), (intent) => intent.issuedAt),
      ...Array.from(this.scalarIntents.values(), (intent) => intent.issuedAt)
    ];
    if (!issuedAt.length) return undefined;
    return Math.max(0, Math.min(...issuedAt) + graceMs - now);
  }

  beginScene(snapshot: PresetSnapshot, scene: number, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginScalar(snapshot, "scene", selectSceneInSnapshot(snapshot, scene), issuedAt);
  }

  beginPresetMove(snapshot: PresetSnapshot, delta: -1 | 1, name?: string, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginScalar(snapshot, "position", movePresetInSnapshot(snapshot, delta, name), issuedAt);
  }

  beginModeSlot(snapshot: PresetSnapshot, slot: 0 | 1 | 2, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginScalar(snapshot, "mode", selectModeSlotInSnapshot(snapshot, slot), issuedAt);
  }

  beginTempo(snapshot: PresetSnapshot, tempo: number, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginScalar(snapshot, "tempo", setTempoInSnapshot(snapshot, tempo), issuedAt);
  }

  beginBypass(snapshot: PresetSnapshot, blockId: string, bypassed: boolean, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginSnapshotMutation(snapshot, setBlockBypassInSnapshot(snapshot, blockId, bypassed), issuedAt);
  }

  beginFootswitch(snapshot: PresetSnapshot, index: number, issuedAt = Date.now()): QcOptimisticCommand {
    return this.beginSnapshotMutation(snapshot, applyFootswitchPreview(snapshot, index), issuedAt, true);
  }

  /** Forget a command that needs no device acknowledgement (for example demo mode). */
  settle(token: QcCommandToken): void {
    for (const rollback of token.scalarRollbacks) {
      if (this.scalarIntents.get(rollback.key)?.id === token.id) this.scalarIntents.delete(rollback.key);
    }
    for (const rollback of token.bypassRollbacks) {
      if (this.bypassIntents.get(rollback.address)?.commandId === token.id) {
        this.bypassIntents.delete(rollback.address);
        this.bypassBases.delete(rollback.address);
      }
    }
  }

  /**
   * Record that the device accepted a command. Its own intent stays pending
   * until the matching echo or refresh arrives; a newer intent that superseded
   * it adopts its result as the value to restore if that newer intent fails.
   */
  acknowledge(token: QcCommandToken, acknowledgedAt = Date.now()): boolean {
    // Commands can complete out of order after their optimistic intent has
    // already been replaced and cleared. A stale success then needs one
    // authoritative refresh even though no intent remains to schedule one.
    const superseded = token.id < this.nextId - 1;
    for (const rollback of token.scalarRollbacks) {
      const intent = this.scalarIntents.get(rollback.key);
      if (!intent || intent.id === token.id || intent.issuedAt < token.issuedAt) continue;
      adoptScalarBase(intent, rollback.applied, acknowledgedAt);
    }
    for (const rollback of token.bypassRollbacks) {
      const intent = this.bypassIntents.get(rollback.address);
      if (!intent || intent.commandId === token.id || intent.issuedAt < token.issuedAt) continue;
      this.adoptBypassBase(rollback.address, rollback.target, acknowledgedAt);
    }
    const footswitch = this.footswitchBase;
    if (footswitch?.commandIds.has(token.id) && token.appliedFootswitchStates
      && this.hasPendingFootswitchCommand(token.id) && acknowledgedAt >= footswitch.observedAt) {
      footswitch.states = token.appliedFootswitchStates;
      footswitch.observedAt = acknowledgedAt;
    }
    return superseded;
  }

  /** Roll back only fields still owned by this command; newer user input wins. */
  fail(snapshot: PresetSnapshot, token: QcCommandToken): PresetSnapshot {
    let next = snapshot;
    for (const rollback of token.scalarRollbacks) {
      const intent = this.scalarIntents.get(rollback.key);
      if (intent?.id !== token.id) continue;
      this.scalarIntents.delete(rollback.key);
      next = restoreScalar(next, intent.base);
    }

    let restoredBypass = false;
    for (const rollback of token.bypassRollbacks) {
      if (this.bypassIntents.get(rollback.address)?.commandId !== token.id) continue;
      this.bypassIntents.delete(rollback.address);
      const base = this.bypassBases.get(rollback.address);
      this.bypassBases.delete(rollback.address);
      restoredBypass = true;
      const previous = base ? base.bypassed : rollback.previous;
      next = {
        ...next,
        blocks: next.blocks.map((block) => block.row === rollback.row && block.column === rollback.column
          ? { ...block, bypassed: previous }
          : block)
      };
    }
    if (restoredBypass && token.previousFootswitchStates) {
      const footswitch = this.footswitchBase;
      next = { ...next, footswitchStates: footswitch?.commandIds.has(token.id) ? footswitch.states : token.previousFootswitchStates };
    }
    return next;
  }

  /** Reconcile and atomically apply one timestamped native device frame. */
  reconcileFrame(
    snapshot: PresetSnapshot,
    states: readonly QcStateUpdate[],
    now = Date.now(),
    graceMs = QC_OPTIMISTIC_COMMAND_GRACE_MS
  ): { snapshot: PresetSnapshot; states: QcStateUpdate[] } {
    const reconciled: QcStateUpdate[] = [];
    let next = snapshot;
    for (const state of states) {
      // A different setlist is a different preset even when its slot matches.
      // Do this before scalar echo filtering: an old position intent cannot
      // suppress an authoritative transition into another setlist.
      if (state.kind === "position" && state.setlistKey !== undefined
        && !samePresetAddress(next, { setlistKey: state.setlistKey, presetPosition: next.presetPosition })) {
        this.observePreset({ setlistKey: state.setlistKey, presetPosition: state.position ?? next.presetPosition });
      }
      const scalar = this.reconcileScalarUpdate(state, now, graceMs);
      if (!scalar) continue;
      if (scalar.kind === "position" && scalar.position !== undefined) {
        this.observePreset({ setlistKey: scalar.setlistKey ?? next.setlistKey, presetPosition: scalar.position });
      }
      const observedAt = scalar.observedAt ?? now;
      const observedBypass = bypassObservations(scalar);
      const bypass = reconcilePendingBypass(scalar, this.bypassIntents, now, graceMs);
      // A contradicting observation suppressed in favor of a newer intent is
      // still the device's value: it becomes what that intent rolls back to.
      for (const [address, bypassed] of observedBypass) {
        if (this.bypassIntents.has(address)) this.adoptBypassBase(address, bypassed, observedAt);
      }
      if (!bypass) continue;
      reconciled.push(bypass);
      next = applyQcStateUpdate(next, bypass);
    }
    return { snapshot: next, states: reconciled };
  }

  /** Merge a slower full read while preserving every newer optimistic command. */
  reconcileSnapshot(
    incoming: PresetSnapshot,
    observedAt = Date.now(),
    now = Date.now(),
    graceMs = QC_OPTIMISTIC_COMMAND_GRACE_MS
  ): PresetSnapshot {
    this.observePreset(incoming);
    let next = reconcilePresetSnapshot(incoming, this.bypassIntents, observedAt, now, graceMs);
    for (const block of incoming.blocks) {
      const address = bypassAddress(block.row, block.column);
      if (this.bypassIntents.has(address)) this.adoptBypassBase(address, block.bypassed, observedAt);
    }
    const footswitch = this.footswitchBase;
    if (footswitch && observedAt >= footswitch.observedAt) {
      footswitch.states = incoming.footswitchStates;
      footswitch.observedAt = observedAt;
    }
    for (const [key, intent] of this.scalarIntents) {
      const actual = scalarValue(next, key);
      if (now - intent.issuedAt >= graceMs || (actual === intent.target && observedAt >= intent.issuedAt)) {
        this.scalarIntents.delete(key);
      } else {
        adoptScalarBase(intent, scalarState(incoming, key), observedAt);
        next = setScalar(next, key, intent.target);
      }
    }
    return next;
  }

  private beginScalar<K extends ScalarKey>(
    before: PresetSnapshot,
    key: K,
    after: PresetSnapshot,
    issuedAt: number
  ): QcOptimisticCommand {
    this.observePreset(before);
    const base = this.scalarIntents.get(key)?.base ?? { ...scalarState(before, key), observedAt: PRE_INTENT };
    const rollback: ScalarRollback = { ...stripObservedAt(base), applied: scalarState(after, key) };
    const token: QcCommandToken = { id: this.nextId++, issuedAt, scalarRollbacks: [rollback], bypassRollbacks: [] };
    this.scalarIntents.set(key, { id: token.id, key, target: scalarValue(after, key), issuedAt, base } as ScalarIntent);
    return { snapshot: after, token };
  }

  private beginSnapshotMutation(
    before: PresetSnapshot,
    after: PresetSnapshot,
    issuedAt: number,
    footswitch = false
  ): QcOptimisticCommand {
    this.observePreset(before);
    const id = this.nextId++;
    // Decide before this command replaces any intent: a superseded press keeps
    // the lamp state the device last confirmed as the one to restore.
    if (footswitch) {
      if (!this.footswitchBase || !this.hasPendingFootswitchCommand()) {
        this.footswitchBase = { states: before.footswitchStates, observedAt: PRE_INTENT, commandIds: new Set() };
      }
      this.footswitchBase.commandIds.add(id);
    }
    const scalarRollbacks: ScalarRollback[] = [];
    for (const key of ["scene", "position", "mode", "tempo"] as const) {
      if (scalarValue(before, key) === scalarValue(after, key)) continue;
      const base = this.scalarIntents.get(key)?.base ?? { ...scalarState(before, key), observedAt: PRE_INTENT };
      scalarRollbacks.push({ ...stripObservedAt(base), applied: scalarState(after, key) });
      this.scalarIntents.set(key, { id, key, target: scalarValue(after, key), issuedAt, base } as ScalarIntent);
    }

    const beforeBlocks = new Map(before.blocks.map((block) => [bypassAddress(block.row, block.column), block]));
    const bypassRollbacks: BypassRollback[] = [];
    for (const block of after.blocks) {
      const address = bypassAddress(block.row, block.column);
      const previous = beforeBlocks.get(address)?.bypassed;
      if (previous === block.bypassed || block.bypassed === undefined) continue;
      if (!this.bypassIntents.has(address)) this.bypassBases.set(address, { bypassed: previous, observedAt: PRE_INTENT });
      bypassRollbacks.push({ address, row: block.row, column: block.column, previous: this.bypassBases.get(address)?.bypassed ?? previous, target: block.bypassed });
      this.bypassIntents.set(address, { bypassed: block.bypassed, issuedAt, commandId: id });
    }
    return {
      snapshot: after,
      token: {
        id,
        issuedAt,
        scalarRollbacks,
        bypassRollbacks,
        previousFootswitchStates: footswitch ? before.footswitchStates : undefined,
        appliedFootswitchStates: footswitch ? after.footswitchStates : undefined
      }
    };
  }

  private hasPendingFootswitchCommand(exceptId?: number): boolean {
    const ids = this.footswitchBase?.commandIds;
    if (!ids) return false;
    for (const intent of this.bypassIntents.values()) {
      if (intent.commandId !== undefined && intent.commandId !== exceptId && ids.has(intent.commandId)) return true;
    }
    return false;
  }

  private adoptBypassBase(address: string, bypassed: boolean | undefined, observedAt: number): void {
    const base = this.bypassBases.get(address);
    if (base && observedAt < base.observedAt) return;
    this.bypassBases.set(address, { bypassed, observedAt });
  }

  private reconcileScalarUpdate(state: QcStateUpdate, now: number, graceMs: number): QcStateUpdate | null {
    let next = state;
    if (state.kind === "scene" && state.activeScene !== undefined
      && !this.acceptScalar("scene", state.activeScene, state, now, graceMs, { key: "scene", previous: state.activeScene })) return null;
    if (state.kind === "position" && state.position !== undefined
      && !this.acceptScalar("position", state.position, state, now, graceMs, { key: "position", previous: state.position })) return null;
    // A mode frame without its slot table cannot describe the complete state
    // a rollback restores, so it is not adopted as a rollback base.
    if (state.kind === "mode" && state.mode && !this.acceptScalar("mode", state.mode, state, now, graceMs,
      state.modeSlots && state.footswitchModes
        ? { key: "mode", previous: state.mode, previousModeSlots: state.modeSlots, previousFootswitchModes: state.footswitchModes }
        : undefined)) return null;
    if ((state.kind === "tempo" || state.kind === "preset") && state.tempo !== undefined
      && !this.acceptScalar("tempo", state.tempo, state, now, graceMs, state.tempoLedEnabled === undefined
        ? undefined
        : { key: "tempo", previous: state.tempo, previousTempoLedEnabled: state.tempoLedEnabled })) {
      if (state.kind === "tempo") return null;
      next = { ...state, tempo: undefined, tempoLedEnabled: undefined };
    }
    return next;
  }

  private acceptScalar<K extends ScalarKey>(
    key: K,
    actual: ScalarValueByKey[K],
    state: QcStateUpdate,
    now: number,
    graceMs: number,
    observed?: ScalarState
  ): boolean {
    const intent = this.scalarIntents.get(key) as ScalarIntent<K> | undefined;
    if (!intent) return true;
    const observedAt = state.observedAt ?? now;
    if (now - intent.issuedAt >= graceMs || (actual === intent.target && observedAt >= intent.issuedAt)) {
      this.scalarIntents.delete(key);
      return true;
    }
    if (observed) adoptScalarBase(intent, observed, observedAt);
    return false;
  }
}

function adoptScalarBase(intent: ScalarIntent, state: ScalarState, observedAt: number): void {
  if (observedAt < intent.base.observedAt) return;
  intent.base = { ...state, observedAt };
}

function stripObservedAt(base: ScalarBase): ScalarState {
  const { observedAt: _observedAt, ...state } = base;
  return state;
}

function scalarState(snapshot: PresetSnapshot, key: ScalarKey): ScalarState {
  return {
    key,
    previous: scalarValue(snapshot, key),
    previousModeSlots: key === "mode" ? snapshot.modeSlots : undefined,
    previousFootswitchModes: key === "mode" ? snapshot.footswitchModes : undefined,
    previousTempoLedEnabled: key === "tempo" ? snapshot.tempoLedEnabled : undefined
  };
}

function bypassObservations(state: QcStateUpdate): Array<[string, boolean | undefined]> {
  if (state.kind === "bypass" && state.row !== undefined && state.column !== undefined && state.bypassed !== undefined) {
    return [[bypassAddress(state.row, state.column), state.bypassed]];
  }
  if (state.kind === "bypassBatch") {
    return (state.bypassUpdates ?? []).map(({ row, column, bypassed }) => [bypassAddress(row, column), bypassed]);
  }
  if (state.kind === "preset") {
    return (state.blocks ?? []).map((block) => [bypassAddress(block.row, block.column), block.bypassed]);
  }
  return [];
}

function scalarValue<K extends ScalarKey>(snapshot: PresetSnapshot, key: K): ScalarValueByKey[K] {
  if (key === "scene") return snapshot.activeScene as ScalarValueByKey[K];
  if (key === "position") return snapshot.presetPosition as ScalarValueByKey[K];
  if (key === "tempo") return snapshot.tempo as ScalarValueByKey[K];
  return snapshot.mode as ScalarValueByKey[K];
}

function setScalar<K extends ScalarKey>(snapshot: PresetSnapshot, key: K, value: ScalarValueByKey[K]): PresetSnapshot {
  if (key === "scene") return selectSceneInSnapshot(snapshot, value as number);
  if (key === "tempo") return setTempoInSnapshot(snapshot, value as number);
  if (key === "mode") {
    const mode = value as PresetSnapshot["mode"];
    // Only a base mode names both footswitch rows. A hybrid's pair is not known from its kind,
    // and an unknown mode has none, so neither is filled in with a guess; the device's own
    // report supplies it (`selectModeSlotInSnapshot` carries it from the configured slot).
    return {
      ...snapshot,
      mode,
      footswitchModes: mode === "PRESET" || mode === "SCENE" || mode === "STOMP" ? [mode, mode] : undefined
    };
  }
  const position = value as number;
  return {
    ...snapshot,
    presetPosition: position,
    presetIdentity: position === snapshot.presetPosition ? snapshot.presetIdentity : undefined,
    presetLocation: `${Math.floor(position / 8) + 1}${String.fromCharCode(65 + (position % 8))}`
  };
}

function restoreScalar(snapshot: PresetSnapshot, rollback: ScalarState): PresetSnapshot {
  const restored = setScalar(snapshot, rollback.key, rollback.previous as never);
  if (rollback.key === "mode") return {
    ...restored,
    modeSlots: rollback.previousModeSlots,
    footswitchModes: rollback.previousFootswitchModes
  };
  if (rollback.key === "tempo") return { ...restored, tempoLedEnabled: rollback.previousTempoLedEnabled };
  return restored;
}
