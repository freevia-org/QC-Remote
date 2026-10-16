import { useId, useState } from "react";
import type { TunerSettings } from "@qc-remote/client";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcUiIcon } from "./theme-icons";
import type { TunerWorkflow } from "./use-tuner-workflow";
import "./official-tuner.css";

const TUNER_INPUTS: ReadonlyArray<[TunerSettings["inputPortId"], string]> = [
  [1, "INPUT 1"], [2, "INPUT 2"], [3, "INPUT 1/2"], [4, "RETURN 1"],
  [5, "RETURN 2"], [8, "USB 5"], [9, "USB 6"]
];

export function CorOsTunerScreen({ workflow, onClose }: { workflow: TunerWorkflow; onClose(): void }) {
  const [inputOpen, setInputOpen] = useState(false);
  const referenceId = useId();
  const settings = workflow.settings;
  const offset = settings?.referenceOffsetHz ?? 0;
  const reference = settings?.referenceHz ?? 440 + offset;
  const muted = settings?.muted ?? false;
  const meterEnabled = settings?.meterEnabled;
  const disabled = workflow.pending || workflow.loading || !workflow.available || !settings;
  const meter = typeof settings?.meter === "number" && Number.isFinite(settings.meter) ? Math.max(-50, Math.min(50, settings.meter)) : undefined;

  const inputLabel = TUNER_INPUTS.find(([value]) => value === (settings?.inputPortId ?? 1))?.[1] ?? "INPUT 1";
  return <section className="coros-tuner tuner-official" aria-label="Tuner">
    <header>
      <span>Tuner</span>
      <button type="button" aria-label="Close Tuner" onClick={onClose}><QcUiIcon kind="check" /></button>
    </header>
    <div className="tuner-scale">
      <span>-50</span>
      {meter !== undefined && <strong className="tuner-reading">{meter.toFixed(1)}</strong>}
      <i />
      {meter !== undefined && <b style={{ transform: `translateX(${meter * 3}px)` }} />}
      <span>+50</span>
    </div>
    <footer>
      <section>
        <label htmlFor={referenceId}>FREQ [Hz]</label>
        <div className="tuner-frequency">
          <QcRotaryDial value={(offset + 30) / 60} origin={0.5} />
          <input id={referenceId} type="range" min={-30} max={30} step={0.1} value={offset} disabled={disabled} aria-label="Tuner reference offset" onChange={(event) => void workflow.setReferenceOffset(Number(event.target.value))} />
          <strong>{reference.toFixed(1)}</strong>
        </div>
      </section>
      <section>
        <button type="button" className="tuner-input-button" disabled={disabled} aria-label="Tuner input" aria-expanded={inputOpen} onClick={() => setInputOpen((value) => !value)}>{inputLabel}<b><QcUiIcon kind="down" /></b></button>
        {inputOpen && <div className="tuner-input-menu" role="menu" aria-label="Tuner inputs">{TUNER_INPUTS.map(([value, label]) => <button type="button" key={value} role="menuitemradio" aria-checked={value === settings?.inputPortId} onClick={() => { setInputOpen(false); void workflow.setInput(value); }}>{label}</button>)}</div>}
      </section>
      <section>
        <button type="button" className={muted ? "tuner-muted" : ""} disabled={disabled} onClick={() => void workflow.setMuted(!muted)}>{muted ? "MUTED" : "MUTE"}</button>
      </section>
      <section>
        <span>LIVE TUNER</span>
        <button type="button" className="tuner-meter-toggle" disabled={disabled || meterEnabled === undefined} aria-pressed={meterEnabled} onClick={() => { if (meterEnabled !== undefined) void workflow.setMeterEnabled(!meterEnabled); }}>
          {meterEnabled !== undefined && <QcPillToggle index={meterEnabled ? 0 : 1} />}
          <span><b>Yes</b></span><span><b>No</b></span>
        </button>
        {meterEnabled === undefined && <small role="status">Live tuner state unavailable.</small>}
      </section>
    </footer>
    {!workflow.available && <p role="status">Tuner settings are unavailable for this device.</p>}
  </section>;
}
