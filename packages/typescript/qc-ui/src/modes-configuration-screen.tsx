import { closeUserOpenedTarget, escapeUserOpenedTarget } from "./user-open-focus";
import { useState } from "react";
import type { ModesConfigurationWorkflow } from "./use-modes-configuration-workflow.ts";
export { useModesConfigurationWorkflow } from "./use-modes-configuration-workflow.ts";
export type { ModesConfigurationWorkflow, ModesConfigurationWorkflowOptions } from "./use-modes-configuration-workflow.ts";
import { mergeModeSlots, QC_MODE_CYCLES, splitModeSlot } from "./device-workflow-model";
import { QcModeGlyph, QcScreenGlyph, QcUiIcon } from "./theme-icons";
import { MODES_CONFIGURATION_INSTRUCTION_LINES } from "./device-reference/device-settings-text";
import { useHoldGesture } from "./use-hold-gesture";
import "./modes-configuration-screen.css";
import "./fixture-live-surface.css";
import "./remaining-fixtures-fixes.css";

function ModeGlyphs({ mode }: { mode: number }) {
  const glyph = (value: number) => <QcModeGlyph mode={(["PRESET", "SCENE", "STOMP"] as const)[value] ?? "PRESET"} />;
  if (mode < 3) return glyph(mode);
  const parts = [[0, 1], [0, 2], [1, 0], [1, 2], [2, 0], [2, 1]][mode - 3] ?? [0, 1];
  return <>{glyph(parts[0])}{glyph(parts[1])}</>;
}

export function ModesConfigurationScreen({ workflow, onClose, returnFocusTarget }: { workflow: ModesConfigurationWorkflow; onClose(): void; returnFocusTarget?: "grid-menu" }) {
  const { slots, loading, pending, refresh, commit } = workflow;
  const [selected, setSelected] = useState<number | null>(null);
  const [announcement, setAnnouncement] = useState("");
  // The long press that splits a hybrid ends with any other gesture: a drag to
  // merge, the pointer leaving, a release or a cancel. Re-arming clears first.
  const hold = useHoldGesture<number>({ onHold: (index) => split(index) });
  const cancelHold = () => hold.cancel();
  const disabled = loading || pending || !slots;
  const split = (index: number) => {
    if (!slots) return;
    const next = splitModeSlot(slots, index);
    if (!next) { setAnnouncement("This hybrid cannot be split while three modes are already in the cycle."); return; }
    setSelected(null); void commit(next);
  };
  const merge = (source: number, target: number) => {
    if (!slots) return;
    const next = mergeModeSlots(slots, source, target);
    if (!next) { setAnnouncement("Only two different base modes can form a hybrid."); return; }
    setSelected(null); void commit(next);
  };
  const activate = (index: number) => {
    if (!slots) return;
    if (selected === null) {
      setSelected(index);
      setAnnouncement(`Selected ${QC_MODE_CYCLES[slots[index]]}. Select another mode to create a hybrid, or drag this mode onto it.`);
    } else if (selected === index && slots[index] >= 3) {
      split(index);
    } else if (selected !== index) {
      merge(selected, index);
    } else {
      setSelected(null); setAnnouncement("Mode selection cleared.");
    }
  };
  return <section className="coros-modes-configuration" aria-label="Modes Configuration" data-qc-open-target="modes-configuration" tabIndex={-1} onKeyDown={(event) => escapeUserOpenedTarget(event, onClose, returnFocusTarget)}>
    <header>
      <span>Modes configuration</span>
      <div>
        <span aria-hidden="true" className="modes-information-glyph">
          <QcScreenGlyph kind="info" />
        </span>
        <button type="button" aria-label="Close Modes Configuration" onClick={(event) => closeUserOpenedTarget(event.currentTarget, onClose, returnFocusTarget)}><QcUiIcon kind="check" /></button>
      </div>
    </header>
    <p>{MODES_CONFIGURATION_INSTRUCTION_LINES[0]}<br />{MODES_CONFIGURATION_INSTRUCTION_LINES[1]}</p>
    <div className="modes-cycle">
      <span>CYCLE</span><i /><i />
      <div aria-label="Modes in cycle">
        {(slots ?? []).map((mode, index) => <button type="button" key={`${mode}-${index}`} disabled={disabled} aria-pressed={selected === index} aria-label={`${QC_MODE_CYCLES[mode] ?? `Unknown mode ${mode}`}, cycle position ${index + 1}. Select another mode to combine.`}
          draggable={!disabled} onClick={() => { if (hold.consumeFired()) return; activate(index); }}
          onDragStart={(event) => { cancelHold(); event.dataTransfer.setData("text/qc-mode-index", String(index)); event.dataTransfer.effectAllowed = "move"; }}
          onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = "move"; }}
          onDrop={(event) => {
            event.preventDefault();
            const sourceText = event.dataTransfer.getData("text/qc-mode-index");
            // An unrelated drag has no mode payload; Number("") would turn
            // it into the first mode and commit an unintended hybrid.
            if (disabled || !/^\d+$/.test(sourceText)) return;
            const source = Number(sourceText);
            if (Number.isInteger(source) && source < (slots?.length ?? 0)) merge(source, index);
          }}
          onPointerDown={() => { if (mode >= 3) hold.start(index); else cancelHold(); }}
          onPointerUp={cancelHold}
          onPointerLeave={cancelHold}
          onPointerCancel={cancelHold}>
          <b><ModeGlyphs mode={mode} /></b>{QC_MODE_CYCLES[mode]?.split(" / ").map((name, part) => <span key={part}>{name[0] + name.slice(1).toLowerCase()}</span>)}
        </button>)}
      </div>
    </div>
    <span className="modes-workflow-a11y" aria-live="polite">{loading ? "Reading Modes Configuration." : announcement || (slots ? `Current mode order: ${slots.map((slot) => QC_MODE_CYCLES[slot]).join(", ")}.` : "Modes Configuration unavailable.")}</span>
    <button type="button" className="modes-refresh-a11y" disabled={loading || pending} aria-label="Refresh Modes Configuration" onClick={() => void refresh()}>Refresh Modes Configuration</button>
  </section>;
}
