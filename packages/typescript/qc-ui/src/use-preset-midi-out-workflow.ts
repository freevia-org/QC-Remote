import { useCallback, useMemo, useRef, useState } from "react";
import type { GatewayTransport, MidiOutMessage, PresetSnapshot } from "@qc-remote/client";
import { requirePresetIdentity } from "@qc-remote/client";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
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
  onSaved?(snapshot: PresetSnapshot): void;
}

/**
 * The Preset MIDI Out writes: saving one source and clearing every source.
 * The editor owns only what is on screen (the chosen source and its draft);
 * the gateway sequencing, its guards and the outcome live here.
 *
 * The editor reports outcomes on its own screen rather than in the app
 * footer, so the shared guarded-mutation policy runs against a local pending
 * flag and posts its refusals and failures to `error`.
 */
export function usePresetMidiOutWorkflow(options: PresetMidiOutWorkflowOptions) {
  const { gateway, snapshot, demo = false, onSaved } = options;
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  // Save and Clear all report separately; one shared flag made the overview
  // announce "MIDI assignments cleared." after an ordinary Save.
  const [status, setStatus] = useState<"saved" | "cleared" | null>(null);
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  const onSavedRef = useRef(onSaved);
  onSavedRef.current = onSaved;
  const report = useCallback((reason: unknown) => setError(reason instanceof Error ? reason.message : String(reason)), []);
  const guarded = useGuardedMutation({ connected: !demo, pending, setPending, notice: setError, fail: report }, "changing MIDI Out");

  const save = useCallback(async (source: PresetMidiOutSource, draft: MidiOutMessage[]) => {
    setError(""); setStatus(null);
    const current = snapshotRef.current;
    await guarded.run({
      write: () => {
        // All twelve rows in the order shown, empty rows as type 0 placeholders,
        // so no message moves (observed on the unit 2026-10-07).
        const messages = midiOutRows(draft);
        return source === "preset"
          ? gateway.setPresetLoadMidiOut(messages, midiOutRows(presetMidiOutSlots(current.presetLoadMidiOut)), current.presetName, requirePresetIdentity(current))
          : gateway.setMidiOut(source, messages, current.presetName, requirePresetIdentity(current));
      },
      settle: (result) => {
        if (result.snapshot) onSavedRef.current?.(result.snapshot);
        setStatus("saved");
      }
    });
  }, [gateway, guarded.run]);

  /**
   * Clears every source that carries messages, one guarded write each, after
   * the user has confirmed. It stops at the first refusal and names the
   * sources already cleared, because those writes stay applied. Each write is
   * guarded with the latest snapshot the device returned, not the render's.
   */
  const clearAll = useCallback(async () => {
    setError(""); setStatus(null);
    const sources: PresetMidiOutSource[] = [...PRESET_MIDI_OUT_SOURCES.keys(), "preset"];
    await guarded.run({
      write: async () => {
        const cleared: string[] = [];
        let current = snapshotRef.current;
        for (const target of sources) {
          const before = presetMidiOutSlots(presetMidiOutSourceMessages(current, target));
          if (!hasActiveMidiOut(before)) continue;
          try {
            const result = target === "preset"
              ? await gateway.setPresetLoadMidiOut([], midiOutRows(before), current.presetName, requirePresetIdentity(current))
              : await gateway.setMidiOut(target, [], current.presetName, requirePresetIdentity(current));
            cleared.push(presetMidiOutSourceLabel(target));
            if (result.snapshot) { current = result.snapshot; onSavedRef.current?.(result.snapshot); }
          } catch (reason) {
            const detail = reason instanceof Error ? reason.message : String(reason);
            throw new Error(`Could not clear ${presetMidiOutSourceLabel(target)}: ${detail} ${cleared.length ? `Already cleared: ${cleared.join(", ")}.` : "Nothing was cleared."}`);
          }
        }
      },
      settle: () => setStatus("cleared")
    });
  }, [gateway, guarded.run]);

  const clearError = useCallback(() => setError(""), []);
  const clearStatus = useCallback(() => setStatus(null), []);

  return useMemo(() => ({ snapshot, demo, pending, error, status, save, clearAll, clearError, clearStatus }),
    [clearAll, clearError, clearStatus, demo, error, pending, save, snapshot, status]);
}

export type PresetMidiOutWorkflow = ReturnType<typeof usePresetMidiOutWorkflow>;
