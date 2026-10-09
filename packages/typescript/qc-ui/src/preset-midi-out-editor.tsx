import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import { useEffect, useMemo, useRef, useState } from "react";
import type { MidiOutMessage, PresetSnapshot } from "@qc-remote/client";
import { QcConfirm } from "./qc-shared-controls";
import { QcDirectoryIcon, QcUiIcon } from "./theme-icons";
import {
  PRESET_MIDI_OUT_SOURCES as SOURCES,
  presetMidiOutSlots as slots,
  presetMidiOutSourceMessages as sourceMessages,
  type PresetMidiOutSource as Source,
  type PresetMidiOutWorkflow
} from "./use-preset-midi-out-workflow.ts";
import "./preset-midi-out.css";

/**
 * What each wire message type is, and what its three values mean. Observed on a
 * CorOS 4.1.0 unit, 2026-10-06 (raw frames written to a scratch preset, then the
 * unit's Preset MIDI Out screen read): 1 is Control Change, 2 is Control Change
 * Toggle and 3 is Program Change. A null label is a value the unit does not show
 * for that type. Only 0..3 may be sent: an undefined type was once followed by
 * the unit restarting.
 */
const MIDI_TYPE_VALUE_LABELS: Readonly<Record<number, readonly [string | null, string | null, string | null]>> = {
  1: ["CC#", "Value", null],
  2: ["CC#", "Min value", "Max value"],
  3: ["Bank CC#0", "Bank CC#32", "Program#"]
};

/** What the editor shows for one source, as a value that changes only when the device's data does. */
function sourceKey(snapshot: PresetSnapshot, source: Source): string {
  return JSON.stringify([snapshot.presetIdentity ?? null, source, slots(sourceMessages(snapshot, source)).map((message) => [message.type, message.channel, message.param1, message.param2, message.param3])]);
}

/**
 * Production Preset MIDI Out editor. The host owns navigation; the writes,
 * their guards and their outcome belong to `usePresetMidiOutWorkflow`.
 */
export function PresetMidiOutEditor({ workflow, onClose, returnFocusTarget }: {
  workflow: PresetMidiOutWorkflow;
  onClose(): void;
  returnFocusTarget?: "grid-menu";
}) {
  const { snapshot, demo, pending, error, status, clearError, clearStatus } = workflow;
  const [source, setSource] = useState<Source>("preset");
  const [detailOpen, setDetailOpen] = useState(false);
  const [draft, setDraft] = useState<MidiOutMessage[]>(() => slots(snapshot.presetLoadMidiOut));
  const [confirmClear, setConfirmClear] = useState(false);
  const snapshotRef = useRef(snapshot);
  snapshotRef.current = snapshot;
  // The host passes a new snapshot object on every reconciled frame (tempo
  // clock, master volume, ...). Resetting on the object threw the user's
  // unsaved draft away on each of them, so the reset is keyed on what the
  // editor shows: the preset, the source, and that source's messages.
  const key = sourceKey(snapshot, source);
  const original = useMemo(() => slots(sourceMessages(snapshotRef.current, source)), [key, source]);
  useEffect(() => { setDraft(original); clearError(); }, [clearError, original]);
  useEffect(() => { clearStatus(); }, [clearStatus, source, snapshot.presetIdentity]);
  const update = (index: number, values: Partial<MidiOutMessage>) => {
    setDraft((current) => current.map((message, item) => item === index ? { ...message, ...values } : message));
    clearStatus();
  };
  const save = () => workflow.save(source, draft);
  const clear = () => { setDraft(slots(undefined)); clearStatus(); };
  const clearAll = () => workflow.clearAll();
  const chooseSource = (next: Source) => { setSource(next); setDetailOpen(true); };

  return <section className="coros-midi-out qc-preset-midi-out" aria-label="Preset MIDI Out" data-qc-open-target="preset-midi-out" tabIndex={-1} onKeyDown={(event) => escapeUserOpenedTarget(event, onClose, returnFocusTarget, pending)}>
    <header>
      <span>Preset MIDI Out settings</span>
      <button type="button" className="midi-trash" aria-label="Clear MIDI assignments" disabled={demo || pending} onClick={() => setConfirmClear(true)}><QcDirectoryIcon kind="trash" /></button>
      <button type="button" aria-label="Close Preset MIDI Out" disabled={pending} onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, returnFocusTarget)}><QcUiIcon kind="check" /></button>
    </header>
    {demo ? <p className="midi-demo-status" role="status">MIDI Out editing is unavailable in Demo. Connect a Quad Cortex to change device data.</p> : !detailOpen ? <>
      <div className="midi-assignment-surface">
        <section className="midi-footswitches" aria-label="Footswitch MIDI assignments">
          <button type="button" className="midi-preset-load" disabled={pending} onClick={() => chooseSource("preset")}>ON PRESET LOAD<br />MESSAGE</button>
          {SOURCES.slice(0, 8).map((label, index) => <button type="button" className="midi-switch" key={label} disabled={pending} onClick={() => chooseSource(index)}><i />{label}</button>)}
        </section>
        <section className="midi-expression" aria-label="Expression MIDI assignments">
          {[8, 9].map((index) => <button type="button" className="midi-expression-source" key={index} aria-label={`Edit EXP ${index - 7} MIDI assignment`} disabled={pending} onClick={() => chooseSource(index)}><span>EXP {index - 7}</span><div><i /></div></button>)}
        </section>
      </div>
      {status === "cleared" && <p className="midi-screen-status" role="status">MIDI assignments cleared.</p>}
      {error && <p className="midi-screen-status" role="alert">{error}</p>}
    </> : <>
      <div className="midi-detail-heading"><button type="button" onClick={() => setDetailOpen(false)}>Back</button><h2>{source === "preset" ? "ON PRESET LOAD MESSAGE" : `${SOURCES[source]} MIDI ASSIGNMENT`}</h2></div>
      <ol className="midi-message-slots" aria-label="MIDI message slots">
        {draft.map((message, index) => <li key={index}>
          <label>Message {index + 1}
            <select aria-label={`Message ${index + 1} type`} value={message.type} disabled={pending} onChange={(event) => update(index, { type: Number(event.target.value) })}>
              <option value={0}>Empty</option><option value={1}>Control Change</option><option value={2}>Control Change Toggle</option><option value={3}>Program Change</option>
            </select>
          </label>
          {message.type !== 0 && <>
            <label>Channel <input aria-label={`Message ${index + 1} channel`} type="number" min={1} max={16} value={message.channel} disabled={pending} onChange={(event) => update(index, { channel: Math.max(1, Math.min(16, Number(event.target.value) || 1)) })} /></label>
            {[1, 2, 3].map((param) => <label key={param}>{MIDI_TYPE_VALUE_LABELS[message.type]?.[param - 1] ?? `Value ${param}`}<input aria-label={`Message ${index + 1} value ${param}`} type="number" min={0} max={127} value={message[`param${param}` as "param1" | "param2" | "param3"]} disabled={pending || MIDI_TYPE_VALUE_LABELS[message.type]?.[param - 1] === null} onChange={(event) => update(index, { [`param${param}`]: Math.max(0, Math.min(127, Number(event.target.value) || 0)) })} /></label>)}
          </>}
        </li>)}
      </ol>
      {error && <p role="alert">{error}</p>}{status === "saved" && <p role="status">MIDI Out saved.</p>}
      <footer className="midi-detail-actions"><button type="button" onClick={clear} disabled={pending}>Clear messages</button><button type="button" onClick={() => setDetailOpen(false)} disabled={pending}>Cancel</button><button type="button" onClick={() => void save()} disabled={pending}>{pending ? "Saving…" : "Save"}</button></footer>
    </>}
    {confirmClear && <QcConfirm message="Clear every MIDI Out message in this preset, on all footswitches, both expression pedals and preset load?" confirmLabel="Clear all" onConfirm={() => { setConfirmClear(false); void clearAll(); }} onCancel={() => setConfirmClear(false)} />}
  </section>;
}
