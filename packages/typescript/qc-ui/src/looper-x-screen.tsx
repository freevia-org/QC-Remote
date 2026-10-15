import { sceneLetter } from "@qc-remote/core";
import { LOOPER_X_ACTIONS } from "./device-workflow-model";
import { QcEditorIcon, QcLooperActionGlyph, QcPresetStackIcon, QcUiIcon } from "./theme-icons";
import type { LooperXWorkflow } from "./use-looper-x-workflow.ts";
import "./looper-x-screen.css";
import "./fixture-live-surface.css";
import "./official-looper-eq.css";

/** The Looper X editor. Its workflow, `useLooperXWorkflow`, lives in use-looper-x-workflow.ts. */
export function LooperXScreen({ workflow, activeScene = 0, onParameters, onMenu, onSelectScene, onClose }: {
  workflow: LooperXWorkflow;
  activeScene?: number;
  onParameters?(): void | Promise<void>;
  onMenu?(): void | Promise<void>;
  onSelectScene?(scene: number): void | Promise<void>;
  onClose(): void;
}) {
  const { status, loading, pending, refresh, run } = workflow;
  const disabled = pending || loading;
  const actions = LOOPER_X_ACTIONS;
  const instruction = <span>USE <b><QcLooperActionGlyph kind="record" /></b> TO START RECORDING</span>;
  return <section className="qc-screen coros-looper" aria-label="Looper X editor">
    <header>
      <button type="button" disabled={disabled || !onMenu} aria-label="Looper X contextual menu" onClick={() => void onMenu?.()}><QcUiIcon kind="more" /></button>
      <span><small>LOOPER</small><strong>Looper X <QcPresetStackIcon /></strong></span>
      <i />
      <button type="button" className="looper-params" disabled={disabled || !onParameters} onClick={() => void onParameters?.()}><QcEditorIcon kind="looper" />Params</button>
      <span className="looper-scene" aria-label={`Scene ${sceneLetter(activeScene)}`}>
        <button type="button" aria-label="Previous scene" disabled={disabled || !onSelectScene} onClick={() => void onSelectScene?.((activeScene + 7) % 8)}><QcEditorIcon kind="scene-previous" /></button>
        <b>{sceneLetter(activeScene)}</b>
        <button type="button" aria-label="Next scene" disabled={disabled || !onSelectScene} onClick={() => void onSelectScene?.((activeScene + 1) % 8)}><QcEditorIcon kind="scene-next" /></button>
      </span>
      <button type="button" disabled={disabled} aria-label="Close Looper X" onClick={onClose}><QcEditorIcon kind="confirm" /></button>
    </header>
    <div className="looper-timeline" aria-label={`Looper status. ${status?.state === undefined ? "Not yet read." : `State ${status.state}.`} ${status?.loopLength === undefined ? "Loop length unavailable." : `Loop length value ${status.loopLength}.`}`}>
      {instruction}
      <span>USE <b className="looper-close-caret"><QcUiIcon kind="close-caret" /></b> TO CLOSE THE LOOPER VIEW</span>
      <em>{status?.freeSamples === undefined ? "AVAILABLE —" : `AVAILABLE ${status.freeSamples} SAMPLES`}</em>
      <span className="looper-refresh-a11y" aria-live="polite">{loading ? "Reading Looper X status." : status ? `Status read. ${status.oneShot === undefined ? "One Shot state unavailable." : `One Shot ${status.oneShot ? "on" : "off"}.`} ${status.halfSpeed === undefined ? "Half Speed state unavailable." : `Half Speed ${status.halfSpeed ? "on" : "off"}.`}` : "Looper X status unavailable."}</span>
      <button type="button" className="looper-status-refresh" disabled={disabled} aria-label="Refresh Looper X status" onClick={() => void refresh()}>Refresh status</button>
    </div>
    <div className="looper-actions">
      {actions.map(([command, label], index) => {
        const glyph = (["duplicate", "one-shot", "half-speed", "punch-in", "record", "play", "reverse", "undo"] as const)[index];
        const key = sceneLetter(index);
        const toggleState = command === "oneShot" ? status?.oneShot : command === "halfSpeed" ? status?.halfSpeed : undefined;
        return <button type="button" key={command} disabled={disabled} aria-pressed={toggleState} onClick={() => void run(command)}>
          <small>{command === "undoRedo" && typeof status?.redoAvailable === "boolean" ? status.redoAvailable ? "REDO" : "UNDO" : label}</small>
          <strong><QcLooperActionGlyph kind={glyph} /></strong><b>{key}</b>
        </button>;
      })}
    </div>
  </section>;
}
