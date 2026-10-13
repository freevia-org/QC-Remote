import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { GatewayTransport, MidiOutMessage, PresetSnapshot } from "@qc-remote/client";
import { requirePresetIdentity } from "@qc-remote/client";
import { connectBeforeNotice, DEVICE_COMMAND_BUSY_NOTICE } from "./use-guarded-mutation.ts";
import { hasActiveMidiOut, MIDI_OUT_SLOT_COUNT, midiOutRows, midiOutSlots } from "./preset-midi-out-rows.ts";

/** The ten MIDI Out sources the unit offers: footswitches A-H and both expression pedals. */
export const PRESET_MIDI_OUT_SOURCES = ["A", "B", "C", "D", "E", "F", "G", "H", "EXP 1", "EXP 2"] as const;
/** Message slots per source. */
export const PRESET_MIDI_OUT_SLOT_COUNT = MIDI_OUT_SLOT_COUNT;

/** A footswitch or expression source index, or the preset-load message list. */
export type PresetMidiOutSource = number | "preset";

/** A source's messages padded to every slot, each message at its own index and empty slots included. */
export const presetMidiOutSlots = midiOutSlots;

export function presetMidiOutSourceMessages(snapshot: PresetSnapshot, source: PresetMidiOutSource): MidiOutMessage[] | undefined {
  return source === "preset" ? snapshot.presetLoadMidiOut : snapshot.midiOut?.find((entry) => entry.source === source)?.messages;
}

export const presetMidiOutSourceLabel = (source: PresetMidiOutSource) => source === "preset" ? "On preset load" : PRESET_MIDI_OUT_SOURCES[source];

export interface PresetMidiOutWorkflowOptions {
  gateway: GatewayTransport;
  snapshot: PresetSnapshot;
  /** The demo device cannot store MIDI Out; every write is refused. */
  demo?: boolean;
  /** Revoke an in-flight write immediately when the host's selected session changes. */
  captureSessionCurrent?(): () => boolean;
  onSaved?(snapshot: PresetSnapshot): void;
}

/**
 * The Preset MIDI Out writes: saving one source and clearing every source.
 * The editor owns only what is on screen (the chosen source and its draft);
 * the gateway sequencing, its guards and the outcome live here.
 *
 * The editor reports outcomes on its own screen rather than in the app
 * footer. The shared refusal wording is preserved, while an immediate local
 * operation owner serializes writes and retires their results, failures and
 * pending state when the selected preset or device session changes.
 */
export function usePresetMidiOutWorkflow(options: PresetMidiOutWorkflowOptions) {
  const { snapshot, demo = false, onSaved } = options;
  // Modal authorization follows the physical host and preset lifetime, not
  // unrelated telemetry objects. A monotonic version also rejects session ABA.
  const editorContextKey = JSON.stringify([demo, snapshot.stateVersion?.session, snapshot.setlistKey, snapshot.presetPosition, snapshot.presetIdentity]);
  const editorOwner = useRef({ key: editorContextKey, gateway: options.gateway, current: options.captureSessionCurrent?.() ?? (() => true), version: 0 });
  if (editorOwner.current.key !== editorContextKey || editorOwner.current.gateway !== options.gateway || !editorOwner.current.current()) {
    editorOwner.current = { key: editorContextKey, gateway: options.gateway, current: options.captureSessionCurrent?.() ?? (() => true), version: editorOwner.current.version + 1 };
  }
  const contextVersion = editorOwner.current.version;
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<{ current(): boolean; error: string; status: "saved" | "cleared" | null }>();
  const error = outcome?.current() ? outcome.error : "";
  // Save and Clear all report separately; one shared flag made the overview
  // announce "MIDI assignments cleared." after an ordinary Save.
  const status = outcome?.current() ? outcome.status : null;
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  const onSavedRef = useRef(onSaved);
  onSavedRef.current = onSaved;
  const latestOptions = useRef(options);
  latestOptions.current = options;
  const mounted = useRef(true);
  type OperationOwner = { current(): boolean; retire(): void; adopt(snapshot: PresetSnapshot): void };
  const inFlight = useRef<OperationOwner | undefined>(undefined);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; inFlight.current?.retire(); inFlight.current = undefined; };
  }, []);
  const operationCurrent = () => {
    const origin = latestOptions.current;
    const hostCurrent = origin.captureSessionCurrent?.() ?? (() => true);
    const identities = new Set([origin.snapshot.presetIdentity]);
    const session = origin.snapshot.stateVersion?.session;
    const setlist = origin.snapshot.setlistKey;
    const position = origin.snapshot.presetPosition;
    let retired = false;
    const current = () => {
      const latest = latestOptions.current;
      return !retired && mounted.current && hostCurrent() && latest.gateway === origin.gateway && Boolean(latest.demo) === Boolean(origin.demo)
        && identities.has(latest.snapshot.presetIdentity) && latest.snapshot.stateVersion?.session === session
        && latest.snapshot.setlistKey === setlist && latest.snapshot.presetPosition === position;
    };
    return { current, retire: () => { retired = true; }, adopt: (snapshot: PresetSnapshot) => { identities.add(snapshot.presetIdentity); } };
  };
  useEffect(() => {
    if (inFlight.current && !inFlight.current.current()) { inFlight.current.retire(); inFlight.current = undefined; setPending(false); }
    if (outcome && !outcome.current()) setOutcome(undefined);
  });
  const run = useCallback(async (status: "saved" | "cleared", write: (owner: OperationOwner) => Promise<void>) => {
    const owner = operationCurrent();
    if (latestOptions.current.demo) {
      setOutcome({ current: owner.current, error: connectBeforeNotice("changing MIDI Out"), status: null }); return;
    }
    if (inFlight.current?.current()) {
      setOutcome({ current: owner.current, error: DEVICE_COMMAND_BUSY_NOTICE, status: null }); return;
    }
    inFlight.current?.retire();
    inFlight.current = owner;
    setPending(true);
    setOutcome(undefined);
    try {
      await write(owner);
      if (owner.current()) setOutcome({ current: owner.current, error: "", status });
    } catch (reason) {
      if (owner.current()) setOutcome({ current: owner.current, error: reason instanceof Error ? reason.message : String(reason), status: null });
    } finally {
      if (inFlight.current === owner) { inFlight.current = undefined; if (mounted.current) setPending(false); }
    }
  }, []);

  const save = useCallback(async (source: PresetMidiOutSource, draft: MidiOutMessage[]) => {
    const current = snapshotRef.current;
    const targetGateway = latestOptions.current.gateway;
    await run("saved", async (owner) => {
        // All twelve rows in the order shown, empty rows as type 0 placeholders,
        // so no message moves (observed on the unit 2026-10-07).
        const messages = midiOutRows(draft);
        const result = source === "preset"
          ? await targetGateway.setPresetLoadMidiOut(messages, midiOutRows(presetMidiOutSlots(current.presetLoadMidiOut)), current.presetName, requirePresetIdentity(current))
          : await targetGateway.setMidiOut(source, messages, current.presetName, requirePresetIdentity(current));
        if (!owner.current()) return;
        if (result.snapshot) { owner.adopt(result.snapshot); onSavedRef.current?.(result.snapshot); }
    });
  }, [run]);

  /**
   * Clears every source that carries messages, one guarded write each, after
   * the user has confirmed. It stops at the first refusal and names the
   * sources already cleared, because those writes stay applied. Each write is
   * guarded with the latest snapshot the device returned, not the render's.
   */
  const clearAll = useCallback(async () => {
    const sources: PresetMidiOutSource[] = [...PRESET_MIDI_OUT_SOURCES.keys(), "preset"];
    const targetGateway = latestOptions.current.gateway;
    await run("cleared", async (owner) => {
        const cleared: string[] = [];
        let current = snapshotRef.current;
        for (const target of sources) {
          if (!owner.current()) return;
          const before = presetMidiOutSlots(presetMidiOutSourceMessages(current, target));
          if (!hasActiveMidiOut(before)) continue;
          try {
            const result = target === "preset"
              ? await targetGateway.setPresetLoadMidiOut([], midiOutRows(before), current.presetName, requirePresetIdentity(current))
              : await targetGateway.setMidiOut(target, [], current.presetName, requirePresetIdentity(current));
            if (!owner.current()) return;
            cleared.push(presetMidiOutSourceLabel(target));
            if (result.snapshot) { current = result.snapshot; owner.adopt(result.snapshot); onSavedRef.current?.(result.snapshot); }
          } catch (reason) {
            const detail = reason instanceof Error ? reason.message : String(reason);
            throw new Error(`Could not clear ${presetMidiOutSourceLabel(target)}: ${detail} ${cleared.length ? `Already cleared: ${cleared.join(", ")}.` : "Nothing was cleared."}`);
          }
        }
    });
  }, [run]);

  const clearError = useCallback(() => setOutcome((value) => value ? { ...value, error: "" } : value), []);
  const clearStatus = useCallback(() => setOutcome((value) => value ? { ...value, status: null } : value), []);

  return useMemo(() => ({ snapshot, demo, pending, error, status, contextVersion, contextCurrent: () => editorOwner.current.version === contextVersion && editorOwner.current.current(), save, clearAll, clearError, clearStatus }),
    [clearAll, clearError, clearStatus, contextVersion, demo, error, pending, save, snapshot, status]);
}

export type PresetMidiOutWorkflow = ReturnType<typeof usePresetMidiOutWorkflow>;
