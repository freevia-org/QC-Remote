import { QC_MAXIMUM_TEMPO_BPM, QC_MINIMUM_TEMPO_BPM, QC_PRESETS_PER_SETLIST, type GridBlock, type GridRoute, type PresetSnapshot, type QcStateUpdate } from "@qc-remote/client";
import { adoptModelPresetCatalog } from "./model-preset-catalog.ts";
import { samePresetAddress } from "./preset-address.ts";
export type { QcStateUpdate } from "@qc-remote/client";

export type PendingBypassIntent = { bypassed: boolean; issuedAt: number; commandId?: number };
export type PendingBypassIntents = Map<string, PendingBypassIntent>;

export function bypassAddress(row: number, column: number): string {
  return `${row}:${column}`;
}

/** Record the latest optimistic state so an older device echo cannot undo it. */
export function markPendingBypass(pending: PendingBypassIntents, row: number, column: number, bypassed: boolean, issuedAt = Date.now()): void {
  pending.set(bypassAddress(row, column), { bypassed, issuedAt });
}

/** Record every bypass transition produced by one optimistic snapshot update. */
export function recordPendingBypassChanges(
  before: PresetSnapshot,
  after: PresetSnapshot,
  pending: PendingBypassIntents,
  issuedAt = Date.now()
): string[] {
  const beforeByAddress = new Map(before.blocks.map((block) => [bypassAddress(block.row, block.column), block]));
  const changed: string[] = [];
  for (const block of after.blocks) {
    const key = bypassAddress(block.row, block.column);
    const previous = beforeByAddress.get(key);
    if (previous && previous.bypassed !== block.bypassed && block.bypassed !== undefined) {
      markPendingBypass(pending, block.row, block.column, block.bypassed, issuedAt);
      changed.push(key);
    }
  }
  return changed;
}

/** Clear only the intents belonging to a failed command, preserving newer taps. */
export function clearPendingBypassChanges(pending: PendingBypassIntents, addresses: readonly string[], issuedAt: number): void {
  for (const address of addresses) {
    if (pending.get(address)?.issuedAt === issuedAt) pending.delete(address);
  }
}

/**
 * Acknowledges matching device state and suppresses contradictory state that
 * predates a newer local command. An expired contradiction is accepted so the
 * UI eventually converges if a hardware command truly failed.
 */
export function reconcilePendingBypass(
  state: QcStateUpdate,
  pending: PendingBypassIntents,
  now = Date.now(),
  graceMs = 3_000,
  observationStartedAt = state.observedAt ?? now
): QcStateUpdate | null {
  const accept = (row: number, column: number, bypassed: boolean): boolean => {
    const key = bypassAddress(row, column);
    const intent = pending.get(key);
    if (!intent) return true;
    // A slow full snapshot may have started before a newer local tap. Even if
    // its old value happens to equal the new target (the common double-toggle
    // ABA case), it cannot acknowledge an intent that did not yet exist.
    if (now - intent.issuedAt >= graceMs || (intent.bypassed === bypassed && observationStartedAt >= intent.issuedAt)) {
      pending.delete(key);
      return true;
    }
    return false;
  };

  if (state.kind === "bypass" && state.row !== undefined && state.column !== undefined && state.bypassed !== undefined) {
    return accept(state.row, state.column, state.bypassed) ? state : null;
  }
  if ((state.kind === "preset" || state.kind === "scene" || state.kind === "bypassBatch") && state.blocks?.length && pending.size) {
    return {
      ...state,
      blocks: state.blocks.map((block) => {
        const key = bypassAddress(block.row, block.column);
        const intent = pending.get(key);
        if (!intent) return block;
        // An explicit projection can withdraw truth. Do not keep an optimistic
        // boolean, or its rollback journal, over authoritative unknown evidence.
        if (state.kind !== "preset" && block.bypassed === undefined) {
          pending.delete(key);
          return block;
        }
        if (now - intent.issuedAt >= graceMs || (intent.bypassed === block.bypassed && observationStartedAt >= intent.issuedAt)) {
          pending.delete(key);
          return block;
        }
        return { ...block, bypassed: intent.bypassed };
      })
    };
  }
  if (state.kind !== "bypassBatch" || !state.bypassUpdates?.length) return state;
  // An explicit projection, including an empty one, supersedes the legacy
  // bool-only list and must not acknowledge unmatched addresses through it.
  if (state.blocks !== undefined) return state;
  const bypassUpdates = state.bypassUpdates.filter(({ row, column, bypassed }) => accept(row, column, bypassed));
  return bypassUpdates.length ? { ...state, bypassUpdates } : null;
}

/** Reconcile every state decoded from one native device frame in wire order. */
export function reconcileQcStateUpdates(
  states: readonly QcStateUpdate[],
  pending: PendingBypassIntents,
  now = Date.now(),
  graceMs = 3_000
): QcStateUpdate[] {
  const reconciled: QcStateUpdate[] = [];
  for (const state of states) {
    const update = reconcilePendingBypass(state, pending, now, graceMs);
    if (update) reconciled.push(update);
  }
  return reconciled;
}

/** Reconcile and reduce one timestamped native frame in one shared operation. */
export function reduceQcStateFrame(
  snapshot: PresetSnapshot,
  states: readonly QcStateUpdate[],
  pending: PendingBypassIntents,
  now = Date.now(),
  graceMs = 3_000
): { snapshot: PresetSnapshot; states: QcStateUpdate[] } {
  const reconciled = reconcileQcStateUpdates(states, pending, now, graceMs);
  return { snapshot: applyQcStateUpdates(snapshot, reconciled), states: reconciled };
}

/** Merge a slower complete read without allowing it to roll back newer taps. */
export function reconcilePresetSnapshot(
  incoming: PresetSnapshot,
  pending: PendingBypassIntents,
  observedAt = Date.now(),
  now = Date.now(),
  graceMs = 3_000
): PresetSnapshot {
  const update = reconcilePendingBypass(
    { kind: "preset", blocks: incoming.blocks, catalogRefresh: true, observedAt },
    pending,
    now,
    graceMs
  );
  return { ...incoming, blocks: update?.blocks ?? incoming.blocks };
}

export function sceneLetter(index: number): string {
  return String.fromCharCode(65 + index);
}

export function selectSceneInSnapshot(snapshot: PresetSnapshot, index: number): PresetSnapshot {
  return { ...snapshot, activeScene: Math.max(0, Math.min(snapshot.scenes.length - 1, index)) };
}

/** The setlist slot one step away, clamped to the setlist like the device clamps it. */
export function adjacentPresetPosition(position: number, delta: -1 | 1): number {
  return Math.max(0, Math.min(QC_PRESETS_PER_SETLIST - 1, position + delta));
}

/** What a step past either end of the setlist reports instead of recalling. */
export function presetBoundaryDetail(delta: -1 | 1): string {
  return `Already at the ${delta > 0 ? "last" : "first"} preset.`;
}

export function movePresetInSnapshot(snapshot: PresetSnapshot, delta: -1 | 1, name?: string): PresetSnapshot {
  const presetPosition = adjacentPresetPosition(snapshot.presetPosition, delta);
  // A step past either end stays on the loaded preset; it names no other one.
  if (presetPosition === snapshot.presetPosition) return snapshot;
  return {
    ...snapshot,
    presetPosition,
    presetIdentity: undefined,
    presetLocation: `${Math.floor(presetPosition / 8) + 1}${sceneLetter(presetPosition % 8)}`,
    presetName: name ?? snapshot.presetName
  };
}

export function setBlockBypassInSnapshot(snapshot: PresetSnapshot, blockId: string, bypassed: boolean): PresetSnapshot {
  return {
    ...snapshot,
    blocks: snapshot.blocks.map((block) => block.id === blockId ? { ...block, bypassed } : block)
  };
}

export function setTempoInSnapshot(snapshot: PresetSnapshot, tempo: number): PresetSnapshot {
  // Only the tempo changes. This used to set `tempoLedEnabled: true` as well, which nothing the
  // device said supported; the LED setting is the device's to report.
  return observed({ ...snapshot, tempo: Math.max(QC_MINIMUM_TEMPO_BPM, Math.min(QC_MAXIMUM_TEMPO_BPM, Math.round(tempo))) }, "tempo");
}

/**
 * Remove `names` from the snapshot's `unobserved` list: a value was just set from something the
 * device reported (or, for an optimistic write, from a command the user issued), so it is no
 * longer the library's placeholder. The list is dropped once it is empty.
 */
function observed(snapshot: PresetSnapshot, ...names: NonNullable<PresetSnapshot["unobserved"]>): PresetSnapshot {
  if (!snapshot.unobserved?.length) return snapshot;
  const remaining = snapshot.unobserved.filter((name) => !names.includes(name));
  if (remaining.length === snapshot.unobserved.length) return snapshot;
  const { unobserved: _removed, ...rest } = snapshot;
  return remaining.length ? { ...rest, unobserved: remaining } : rest;
}

/** The label a scene is shown with: an empty or blank stored label reads `Scene A` to `Scene H`. */
function sceneDisplayLabel(index: number, stored: string | undefined): string {
  return stored?.trim() ? stored : `Scene ${sceneLetter(index)}`;
}

export function selectModeSlotInSnapshot(snapshot: PresetSnapshot, slot: 0 | 1 | 2): PresetSnapshot {
  const selected = snapshot.modeSlots?.find((entry) => entry.slot === slot);
  if (!selected) return snapshot;
  type BaseMode = NonNullable<PresetSnapshot["footswitchModes"]>[number];
  const baseModes = new Set<BaseMode>(["PRESET", "SCENE", "STOMP"]);
  const isBaseMode = (mode: string): mode is BaseMode => baseModes.has(mode as BaseMode);
  const pair = selected.label.split(" / ");
  if (selected.mode === "HYBRID" && pair.length === 2 && pair.every(isBaseMode)) {
    return { ...snapshot, mode: selected.mode, footswitchModes: pair };
  }
  if (isBaseMode(selected.mode) && selected.label === selected.mode) {
    return { ...snapshot, mode: selected.mode, footswitchModes: [selected.mode, selected.mode] };
  }
  return snapshot;
}

/** An active hybrid slot needs the observed ordered pair, not just HYBRID kind. */
export function modeSlotIsActive(snapshot: PresetSnapshot, slot: 0 | 1 | 2): boolean {
  // Reuse the existing slot projection: no second raw-mode/label decoder.
  const selected = selectModeSlotInSnapshot(snapshot, slot);
  if (selected === snapshot || selected.mode !== snapshot.mode) return false;
  if (snapshot.mode !== "HYBRID") return true;
  const expected = selected.footswitchModes;
  const observed = snapshot.footswitchModes;
  return expected !== undefined && observed !== undefined && expected.length === observed.length
    && expected.every((mode, index) => mode === observed[index]);
}

/** Native revisions compare reductions within one session, never across sessions. */
export function canAdoptNativeState(current: PresetSnapshot["stateVersion"], incoming: PresetSnapshot["stateVersion"]): boolean {
  if (!incoming) return !current;
  if (!incoming.session || !Number.isSafeInteger(incoming.revision) || incoming.revision < 1) return false;
  return !current || (incoming.session === current.session && incoming.revision >= current.revision);
}

export function applyQcStateUpdate(snapshot: PresetSnapshot, state: QcStateUpdate): PresetSnapshot {
  if (snapshot.stateVersion && snapshot.stateVersion.session !== state.stateVersion?.session) return snapshot;
  const next = reduceQcStateUpdate(snapshot, state);
  const version = canAdoptNativeState(snapshot.stateVersion, state.stateVersion) ? state.stateVersion : snapshot.stateVersion;
  return version ? { ...next, stateVersion: version } : next;
}

function reduceQcStateUpdate(snapshot: PresetSnapshot, state: QcStateUpdate): PresetSnapshot {
  if (state.kind === "scene" && state.activeScene !== undefined) {
    const next = { ...selectSceneInSnapshot(snapshot, state.activeScene),
      footswitchStates: state.footswitchStates ?? snapshot.footswitchStates,
      routes: snapshot.routes.map((route) => {
        const observed = state.routes?.find((item) => item.row === route.row && item.inputId === route.inputId
          && item.outputId === route.outputId && item.splitColumn === route.splitColumn
          && item.mixColumn === route.mixColumn && item.splitterModelId === route.splitterModelId);
        return { ...route, splitMuted: observed?.splitMuted };
      }) };
    if (!state.blocks) return next;
    return { ...next, blocks: next.blocks.map((block) => {
      const observed = state.blocks!.find((item) => item.row === block.row && item.column === block.column
        && item.modelId !== undefined && item.modelId === block.modelId);
      return { ...block, bypassed: observed?.bypassed };
    }) };
  }
  if (state.kind === "dirty" && state.dirty !== undefined) return observed({ ...snapshot, dirty: state.dirty }, "dirty");
  if (state.kind === "master" && state.masterVolume !== undefined) {
    // The contract says a master update is normalized, 0 to 1; a value above 1 is read as a
    // percentage already, which is a convenience for callers that publish one. The two readings
    // cannot be told apart for a value of exactly 1 (100% normalized, or 1%), and the scale of
    // this field is being settled with the other volume and tempo scales.
    const volume = state.masterVolume <= 1 ? state.masterVolume * 100 : state.masterVolume;
    return observed({ ...snapshot, masterVolume: Math.round(volume) }, "masterVolume");
  }
  if (state.kind === "mode" && state.mode) return observed({
    ...snapshot,
    mode: state.mode,
    rawMode: state.rawMode,
    modeSlots: state.modeSlots ?? snapshot.modeSlots,
    // The pair belongs to the mode it was reported with; an update that reports none (an unknown
    // mode) clears the old pair instead of leaving the previous mode's split in place.
    footswitchModes: state.footswitchModes
  }, "mode");
  if (state.kind === "sceneLabel" && state.index !== undefined && state.label !== undefined) {
    // The same display rule a full preset applies; the stored text is kept as it came.
    return {
      ...snapshot,
      scenes: snapshot.scenes.map((label, index) => index === state.index ? sceneDisplayLabel(index, state.label) : label),
      sceneLabelsRaw: snapshot.sceneLabelsRaw?.map((label, index) => index === state.index ? state.label! : label)
    };
  }
  if (state.kind === "sceneColor" && state.index !== undefined && state.color) {
    const colors = [...(snapshot.sceneColors ?? [])];
    colors[state.index] = state.color;
    const argb = snapshot.sceneColorsArgb && state.rawColor !== undefined
      ? snapshot.sceneColorsArgb.map((value, index) => index === state.index ? state.rawColor! : value)
      : snapshot.sceneColorsArgb;
    return { ...snapshot, sceneColors: colors, sceneColorsArgb: argb };
  }
  if (state.kind === "bypass" && state.row !== undefined && state.column !== undefined && state.bypassed !== undefined) {
    return {
      ...snapshot,
      blocks: snapshot.blocks.map((block) => block.row === state.row && block.column === state.column ? { ...block, bypassed: state.bypassed } : block)
    };
  }
  if (state.kind === "bypassBatch" && state.blocks) {
    return { ...snapshot, footswitchStates: state.footswitchStates ?? snapshot.footswitchStates, blocks: snapshot.blocks.map((block) => {
      const observed = state.blocks!.find((item) => item.row === block.row && item.column === block.column
        && item.modelId !== undefined && item.modelId === block.modelId);
      return observed ? { ...block, bypassed: observed.bypassed } : block;
    }) };
  }
  if (state.kind === "bypassBatch" && state.bypassUpdates?.length) {
    const updates = new Map(state.bypassUpdates.map((update) => [bypassAddress(update.row, update.column), update.bypassed]));
    return {
      ...snapshot,
      blocks: snapshot.blocks.map((block) => updates.has(bypassAddress(block.row, block.column)) ? { ...block, bypassed: updates.get(bypassAddress(block.row, block.column))! } : block)
    };
  }
  if (state.kind === "tempo") return observed({
    ...snapshot,
    tempo: state.tempo ?? snapshot.tempo,
    tempoLedEnabled: state.tempoLedEnabled ?? snapshot.tempoLedEnabled
  }, ...(state.tempo !== undefined ? ["tempo" as const] : []), ...(state.tempoLedEnabled !== undefined ? ["tempoLedEnabled" as const] : []));
  if (state.kind === "ioPorts" && state.ioSettingsError) {
    return { ...snapshot, ioPorts: undefined, ioSettingsError: state.ioSettingsError };
  }
  if (state.kind === "ioPorts" && snapshot.ioSettingsError) return snapshot;
  if (state.kind === "ioPorts" && state.ioPorts?.length) {
    const ports = [...(snapshot.ioPorts ?? [])];
    for (const update of state.ioPorts) {
      const index = ports.findIndex((port) => port.kind === update.kind && port.id === update.id);
      if (index >= 0) ports[index] = update;
      else ports.push(update);
    }
    return { ...snapshot, ioPorts: ports };
  }
  if (state.kind === "midiOut") return {
    ...snapshot,
    midiOut: state.midiOut ? [
      ...(snapshot.midiOut ?? []).filter((source) => !state.midiOut!.some((update) => update.source === source.source)),
      ...state.midiOut.filter((source) => source.messages.length > 0)
    ].sort((a, b) => a.source - b.source) : snapshot.midiOut,
    presetLoadMidiOut: state.presetLoadMidiOut ?? snapshot.presetLoadMidiOut
  };
  if (state.kind === "position" && state.position !== undefined) {
    const bank = Math.floor(state.position / 8) + 1;
    const setlistName = state.setlistKey?.split("/").filter(Boolean).at(-1) ?? snapshot.setlistName;
    return observed({
      ...snapshot,
      presetPosition: state.position,
      presetIdentity: state.presetIdentity ?? (samePresetAddress(snapshot, {
        setlistKey: state.setlistKey ?? snapshot.setlistKey, presetPosition: state.position
      }) ? snapshot.presetIdentity : undefined),
      presetLocation: `${bank}${sceneLetter(state.position % 8)}`,
      setlistKey: state.setlistKey ?? snapshot.setlistKey,
      setlistName
    }, "presetLocation");
  }
  if (state.kind === "blockRemoved" && state.row !== undefined && state.column !== undefined) {
    return {
      ...snapshot,
      blocks: snapshot.blocks.filter((block) => block.row !== state.row || block.column !== state.column),
      footswitchStates: state.footswitchStates ?? snapshot.footswitchStates,
      gridLayoutComplete: state.gridLayoutComplete ?? snapshot.gridLayoutComplete
    };
  }
  if (state.kind === "gridTopology") return {
    ...snapshot,
    ...(state.refreshRequired ? { presetIdentity: undefined } : {}),
    gridLayoutComplete: state.gridLayoutComplete ?? false
  };
  // The projection is the host's whole fold, so it is kept beside the preset, not merged into it.
  // Nothing about the current preset or its Grid changes.
  if (state.kind === "modelPresetCatalog") {
    const modelPresetCatalog = adoptModelPresetCatalog(snapshot.modelPresetCatalog, state.modelPresetCatalog);
    return modelPresetCatalog === snapshot.modelPresetCatalog ? snapshot : { ...snapshot, modelPresetCatalog };
  }
  if (state.kind === "preset") return observed({
    ...snapshot,
    presetIdentity: state.presetIdentity && state.position !== undefined && state.setlistKey !== undefined
      ? (samePresetAddress(snapshot, { setlistKey: state.setlistKey, presetPosition: state.position }) ? state.presetIdentity : undefined)
      : snapshot.presetIdentity,
    // Full preset publications include ordinary reads. Only an explicit dirty
    // observation can establish whether the device has unsaved edits.
    dirty: state.dirty ?? snapshot.dirty,
    presetName: state.presetName ?? snapshot.presetName,
    tempo: state.tempo ?? snapshot.tempo,
    tempoLedEnabled: state.tempoLedEnabled ?? snapshot.tempoLedEnabled,
    scenes: state.scenes?.length ? state.scenes : snapshot.scenes,
    sceneLabelsRaw: state.sceneLabelsRaw?.length ? state.sceneLabelsRaw : snapshot.sceneLabelsRaw,
    sceneColors: state.sceneColors?.length ? state.sceneColors : snapshot.sceneColors,
    sceneColorsArgb: state.sceneColorsArgb?.length ? state.sceneColorsArgb : snapshot.sceneColorsArgb,
    blocks: state.blocks ?? snapshot.blocks,
    footswitchStates: state.footswitchStates ?? snapshot.footswitchStates,
    gridLayoutComplete: state.gridLayoutComplete ?? snapshot.gridLayoutComplete,
    routes: state.routes ?? snapshot.routes
  }, ...(state.dirty !== undefined ? ["dirty" as const] : []), ...(state.tempo !== undefined ? ["tempo" as const] : []), ...(state.tempoLedEnabled !== undefined ? ["tempoLedEnabled" as const] : []));
  return snapshot;
}

/** Apply one native frame with a single immutable snapshot commit. */
export function applyQcStateUpdates(snapshot: PresetSnapshot, states: readonly QcStateUpdate[]): PresetSnapshot {
  return states.reduce(applyQcStateUpdate, snapshot);
}
