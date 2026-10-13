import { useState } from "react";
import type { GlobalEqSettings } from "@qc-remote/client";
import { globalEqUnavailableNotice } from "@qc-remote/core";
import { QcIoJack } from "./qc-io-jack";
import { IoReading } from "./io-reading";
import { QcLevelMeter } from "./qc-level-meter";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryDial } from "./qc-rotary-dial";
import { EQ_FILTER_TYPE_LABELS } from "./parameter-model";
import { formatGlobalEqFrequency, globalEqQ } from "./global-eq-scale";
import { QcEditorIcon, QcEqIcon, QcIoIcon, QcPresetStackIcon, QcScreenGlyph, QcScreenVectorGlyph, QcUiIcon } from "./theme-icons";
import type { IoEqWorkflow } from "./use-io-eq-workflows";
import type { IoView } from "./io-unit-selection.ts";
import "./qc-screen-elements.css";
import "./fixture-live-surface.css";
import "./official-io.css";
import "./official-looper-eq.css";
import "./io-global-eq-screens.css";

type Workflow = IoEqWorkflow;
const IO_PORTS: Array<{ id: IoView; label: string; sub: string; kind?: "square" | "midi" | "input" | "combo"; paired?: boolean; sockets: string[] }> = [
  { id: "usb", label: "", sub: "USB", kind: "square", sockets: ["usb"] },
  { id: "expression", label: "EXP 2", sub: "EXP 1", paired: true, sockets: ["exp2", "exp1"] },
  { id: "midi", label: "", sub: "MIDI OUT", kind: "midi", sockets: ["midi-out"] },
  { id: "midi", label: "", sub: "MIDI IN", kind: "midi", sockets: ["midi-in"] },
  { id: "output", label: "", sub: "OUT 2/R", kind: "combo", sockets: ["out2"] },
  { id: "output", label: "", sub: "OUT 1/L", kind: "combo", sockets: ["out1"] },
  { id: "headphones", label: "", sub: "CAPTURE OUT", paired: true, sockets: ["phones", "capture-out"] },
  { id: "overview", label: "OUT 4/R", sub: "OUT 3/L", paired: true, sockets: ["out4", "out3"] },
  { id: "return", label: "RET 2", sub: "RET 1", paired: true, sockets: ["ret2", "ret1"] },
  { id: "send-return", label: "SEND 2", sub: "SEND 1", paired: true, sockets: ["send2", "send1"] },
  { id: "input", label: "INPUT 2", sub: "", kind: "input", sockets: ["input2"] },
  { id: "input", label: "INPUT 1", sub: "", kind: "input", sockets: ["input1"] }
];
const ACTIVE_SOCKETS: Record<IoView, string[]> = { usb: ["usb"], input: ["input1"], output: ["out1", "out2"], overview: ["out3", "out4"], "send-return": ["send1", "send2"], return: ["ret1", "ret2"], expression: ["exp1", "exp2"], midi: ["midi-in", "midi-out"], capture: ["capture-out"], headphones: ["phones"] };
/** The input view edits whichever input jack was chosen, so its ring follows `inputId`. */
const activeSocketsFor = (view: IoView, inputId: number) => view === "input" ? [`input${inputId}`] : ACTIVE_SOCKETS[view];
const clamp = (n: number, min = 0, max = 1) => Math.min(max, Math.max(min, n));
const eqValue = (eq: GlobalEqSettings | undefined, index: number, fallback = 0) => eq?.parameters.find((p) => p.parameterIndex === index)?.value ?? fallback;

function PortState({ socket, active, plugged, kind }: { socket: string; active: string[]; plugged: readonly string[]; kind: string | undefined }) {
  const jackKind = kind === "square" ? "usb" : kind === "midi" ? "midi" : kind === "combo" ? "combo" : kind === "input" ? "input" : "jack";
  return <QcIoJack kind={jackKind} state={active.includes(socket) ? "active" : plugged.includes(socket) ? "plugged" : "idle"} />;
}

function Dial({ label, value, display, min, max, disabled, onChange, steps, accent }: { label: string; value: number; display: string; min: number; max: number; disabled: boolean; onChange?(value: number): void; steps?: number; accent?: string }) {
  const position = clamp((value - min) / (max - min));
  return <span className="io-dial-wrap">
    <QcRotaryDial className={`io-dial${steps ? " is-stepped" : ""}`} value={position} mode={steps ? "step" : "arc"} steps={steps} accent={accent} />
    <strong><IoReading text={display} /></strong>
    <input className="ioeq-dial-input" aria-label={label} type="range" min={min} max={max} step={steps ? (max - min) / Math.max(1, steps - 1) : (max - min) / 100} value={value} disabled={disabled} onChange={onChange ? (event) => onChange(Number(event.target.value)) : undefined} />
  </span>;
}

function PillSwitch({ label, value, disabled, onChange, options = ["On", "Off"], leftMeansTrue = true, as: Tag = "section" }: { label: string; value: boolean; disabled: boolean; onChange(value: boolean): void; options?: [string, string]; leftMeansTrue?: boolean; as?: "section" | "span" }) {
  const leftSelected = value === leftMeansTrue;
  return <Tag className={`io-switch${disabled ? " is-disabled" : ""}`}>
    <em>{label}</em><QcPillToggle index={leftSelected ? 0 : 1} />
    <button type="button" disabled={disabled} aria-pressed={leftSelected} onClick={() => onChange(leftMeansTrue)}>{options[0]}</button>
    <button type="button" disabled={disabled} aria-pressed={!leftSelected} onClick={() => onChange(!leftMeansTrue)}>{options[1]}</button>
  </Tag>;
}

function IoDialControl({ label, value, display, min, max, disabled, onChange, steps }: { label: string; value: number; display: string; min: number; max: number; disabled: boolean; onChange(value: number): void; steps?: number }) {
  return <section><span>{label}</span><Dial label={label} value={value} display={display} min={min} max={max} disabled={disabled} onChange={onChange} steps={steps} /></section>;
}

/** Device-layout I/O screen backed by the shared CorOS graphics and rotary/toggle controls. */
export function CorOsIoSettingsScreen({ workflow, onGlobalEq, onClose, initialView = "overview" }: { workflow: Workflow; onGlobalEq(): void; onClose?: () => void; initialView?: IoView }) {
  // The unit's own I/O screen decides which port or pair is shown: on open (the first render already has it) and
  // whenever it changes while open. The app's own taps are mirrored to the unit and come back as the same selection.
  const unit = workflow.unitSelection;
  const [view, setView] = useState<IoView>(unit?.view ?? initialView);
  const plugged = unit?.plugged ?? ["usb"];
  const [inputId, setInputId] = useState(unit?.inputId ?? 1);
  const [paneGeneration, setPaneGeneration] = useState(0);
  const [followed, setFollowed] = useState(unit);
  if (unit !== followed) {
    setFollowed(unit);
    if (unit?.view && (unit.view !== view || (unit.view === "input" && unit.inputId !== undefined && unit.inputId !== inputId))) {
      setView(unit.view);
      if (unit.inputId !== undefined) setInputId(unit.inputId);
      setPaneGeneration((value) => value + 1);
    }
  }
  const { io, loading, pending } = workflow;
  const unavailable = loading || pending || !io;
  const activeSockets = activeSocketsFor(view, inputId);
  const title = view === "input" ? `Input ${inputId}` : view === "output" ? "Output 1/2" : view === "overview" ? "Output 3/4" : view === "send-return" ? "Send 1/2" : view === "return" ? "Return 1/2" : view === "expression" ? "Expression ports" : view === "midi" ? "MIDI" : view === "capture" ? "Capture Out" : view === "usb" ? "USB" : "Headphones";
  const input = io?.inputs.find((p) => p.inputPortId === inputId);
  const outputIds = view === "send-return" ? [8, 9] : view === "overview" ? (io?.out34Linked ? [2] : [6, 7]) : (io?.xlr12Linked ? [1] : [4, 5]);
  const outputs = outputIds.map((id) => io?.outputs.find((p) => p.outputPortId === id)).filter((p) => p !== undefined);
  const setInput = (patch: Partial<NonNullable<typeof input>>) => void workflow.setInputPort(inputId, patch.levelDb ?? null, patch.impedance ?? null, patch.inputType ?? null, patch.groundLift ?? null);
  return <section className={`qc-screen coros-io-settings is-${view}`} data-qc-screen-context={paneGeneration} aria-label={`I/O Settings ${title}`}>
    <header><span className="io-heading"><i aria-hidden="true"><QcScreenVectorGlyph icon="io.header" /></i><span><small>I/O SETTINGS</small><strong>{title}</strong></span></span>
      <button type="button" className="io-global-eq" data-qc-screen-navigation onClick={onGlobalEq}>GLOBAL EQ<i aria-hidden="true" className={workflow.eq?.bypassed === false ? "is-on" : undefined} /></button><button type="button" aria-label="Close I/O Settings" disabled={!onClose} onClick={onClose}><QcUiIcon kind="check" /></button></header>
    <div className="io-ports">{IO_PORTS.map((port, index) => {
      const active = port.sockets.some((socket) => activeSockets.includes(socket));
      return <button type="button" key={`${port.label}-${index}`} data-qc-screen-navigation className={`${active ? "is-active" : ""} is-${port.kind ?? "jack"}${port.paired ? " is-paired" : ""}`} aria-current={active ? "page" : undefined} onClick={(event) => { const capture = port.id === "headphones" && ((event.target as HTMLElement).closest("[data-io-socket]")?.getAttribute("data-io-socket") === "capture-out" || (event.target as HTMLElement).tagName === "SMALL"); setPaneGeneration(value => value + 1); setView(capture ? "capture" : port.id); if (port.kind === "input") setInputId(port.sockets[0] === "input2" ? 2 : 1); }}>
        <span className={port.id === "headphones" ? "io-headphone-label" : undefined}>{port.id === "headphones" ? <QcIoIcon kind="headphones-symbol" className={view === "headphones" ? "is-active" : "is-muted"} /> : port.label}</span>
        {port.sockets.map((socket) => <i key={socket} data-io-socket={socket}><PortState socket={socket} active={activeSockets} plugged={plugged} kind={port.kind} /></i>)}<small>{port.sub}</small>
      </button>;
    })}<button type="button" className="io-port-link io-port-link-output12" aria-label="Toggle XLR 1/2 pairing" aria-pressed={Boolean(io?.xlr12Linked)} disabled={unavailable || io?.xlr12Linked === undefined} onClick={() => void workflow.setOutputPairing(!io?.xlr12Linked, null)}><QcScreenVectorGlyph icon="io.linked" /></button><button type="button" className="io-port-link io-port-link-output34" aria-label="Toggle outputs 3/4 pairing" aria-pressed={Boolean(io?.out34Linked)} disabled={unavailable || io?.out34Linked === undefined} onClick={() => void workflow.setOutputPairing(null, !io?.out34Linked)}><QcScreenVectorGlyph icon="io.linked" /></button>
      {view === "usb" && <div className="io-input-selectors" aria-label="USB dry and wet channel assignment">
        <button type="button" className={(io?.usb?.dryWet ?? 0) < .5 ? "is-active" : ""} aria-pressed={(io?.usb?.dryWet ?? 0) < .5} aria-label="USB 1 and 2 dry, USB 3 and 4 wet" disabled={unavailable || io?.usb?.dryWet === undefined} onClick={() => void workflow.setUsbPort(null, null, 0)}>1</button>
        <button type="button" className={(io?.usb?.dryWet ?? 0) >= .5 ? "is-active" : ""} aria-pressed={(io?.usb?.dryWet ?? 0) >= .5} aria-label="USB 3 and 4 dry, USB 1 and 2 wet" disabled={unavailable || io?.usb?.dryWet === undefined} onClick={() => void workflow.setUsbPort(null, null, 1)}>2</button>
      </div>}
    </div>
    {view === "input" ? <div className="io-editor is-analog"><section><span>{`IN ${inputId} LEVEL`}</span><Dial label={`Input ${inputId} level`} value={input?.levelDb ?? 0} display={`${(input?.levelDb ?? 0).toFixed(1)} dB`} min={-12} max={60} disabled={unavailable || input?.levelDb === undefined} onChange={(levelDb) => setInput({ levelDb })} /></section>
      <section><span>IMPEDANCE</span><Dial label={`Input ${inputId} impedance`} value={input?.impedance ?? 1} display={(["10k", "33k", "68k", "91k", "130k", "232k", "470k", "1M", "10M"][Math.round((input?.impedance ?? 1) * 8)] ?? "—") + " Ω"} min={0} max={1} disabled={unavailable || input?.impedance === undefined} onChange={(impedance) => setInput({ impedance: Math.round(impedance * 8) / 8 })} steps={9} /></section>
      <PillSwitch label="TYPE" value={input?.inputType === 0} options={["Mic", "Instrument"]} leftMeansTrue={false} disabled={unavailable || input?.inputType === undefined} onChange={(value) => setInput({ inputType: value ? 0 : 0.5 })} />
      <PillSwitch label="PHANTOM 48V" value={input?.inputType === 1} disabled={unavailable || input?.inputType === undefined} onChange={(value) => setInput({ inputType: value ? 1 : 0.5 })} />
      <PillSwitch label="GROUND LIFT" value={Boolean(input?.groundLift)} disabled={unavailable || input?.groundLift === undefined} onChange={(groundLift) => setInput({ groundLift: groundLift ? 1 : 0 })} />
      <div className="io-level-meter"><span>{`IN ${inputId} LEVEL`}</span><strong>—</strong><QcLevelMeter /></div></div>
      : view === "output" || view === "overview" || view === "send-return" ? <div className="io-editor is-output">{outputs.map((port) => <section key={port.outputPortId}>
        <span><em>{view === "send-return" ? `SEND ${port.outputPortId - 7}` : `OUT ${port.outputPortId === 1 ? "1/2" : port.outputPortId === 2 ? "3/4" : port.outputPortId - 3} LEVEL`}</em><Dial label={`Output ${port.outputPortId} level`} value={port.levelDb ?? 0} display={`${(port.levelDb ?? 0).toFixed(1)} dB`} min={-40} max={12} disabled={unavailable || port.levelDb === undefined} onChange={(level) => void workflow.setOutputPort(port.outputPortId, level, null, null)} /></span>
        {view === "output" && <PillSwitch as="span" label="GROUND LIFT" value={Boolean(port.groundLift)} disabled={unavailable || port.groundLift === undefined} onChange={(value) => void workflow.setOutputPort(port.outputPortId, null, value ? 1 : 0, null)} />}
        <span className="io-readout"><em>{view === "send-return" ? `SEND ${port.outputPortId - 7} LEVEL` : "OUT LEVEL"}</em><strong>{port.levelDb === undefined ? "—" : <IoReading text={`${port.levelDb.toFixed(1)} dB`} />}</strong><QcLevelMeter /></span><span className="io-readout"><em>LIMITER</em><strong>—</strong><QcLevelMeter /></span>
        {view === "overview" && <span />}{view !== "send-return" && <PillSwitch as="span" label="MUTE" value={Boolean(port.muted)} disabled={unavailable || port.muted === undefined} onChange={(mute) => void workflow.setOutputPort(port.outputPortId, null, null, mute)} />}
      </section>)}</div>
      : view === "usb" ? <div className="io-editor is-usb"><IoDialControl label="USB LEVEL" value={io?.usb?.level ?? 0} display={`${(io?.usb?.level ?? 0).toFixed(2)}`} min={0} max={1} disabled={unavailable || io?.usb?.level === undefined} onChange={(level) => void workflow.setUsbPort(level, null, null)} />
        <IoDialControl label="HP SOURCE" value={(io?.usb?.headphonesSource ?? 0) * 3} display={(["NONE", "USB 1/2", "USB 3/4", "BOTH"][Math.round((io?.usb?.headphonesSource ?? 0) * 3)])} min={0} max={3} steps={4} disabled={unavailable || io?.usb?.headphonesSource === undefined} onChange={(source) => void workflow.setUsbPort(null, Math.round(source) / 3, null)} />
        <div className="io-meter-grid">{["IN 1/2", "IN 3/4", "IN 5/6", "IN 7/8", "OUT 1/2", "OUT 3/4", "OUT 5/6", "OUT 7/8"].map((name) => <span key={name}><b>{name}</b><i><QcScreenGlyph kind="info" /></i><small><span>— dB</span><span>—</span></small><QcLevelMeter /><QcLevelMeter /></span>)}</div>
      </div>
      : view === "return" ? <div className="io-editor is-output">{[4, 5].map((id) => {
        const port = io?.inputs.find((entry) => entry.inputPortId === id);
        return <section key={id}><IoDialControl label={`Return ${id - 3} level`} value={port?.levelDb ?? 0} display={port?.levelDb === undefined ? "—" : `${port.levelDb.toFixed(1)} dB`} min={-12} max={60} disabled={unavailable || port?.levelDb === undefined} onChange={(level) => void workflow.setInputPort(id, level, null, null, null)} /></section>;
      })}</div>
      : view === "midi" ? <div className="io-editor is-analog"><PillSwitch label="MIDI THRU" value={(io?.midi?.thru ?? 0) >= .5} disabled={unavailable || io?.midi?.thru === undefined} onChange={(enabled) => void workflow.setMidiThru(enabled)} /></div>
      : view === "expression" ? <div className="io-editor is-headphones">{io?.expressionPorts.map((port) => <section key={port.expressionPortId}><span>{`Observed expression port ${port.expressionPortId}`}</span><strong>{port.plugged === undefined ? "Connection unknown" : port.plugged ? "Connected" : "Not connected"}</strong><small>{port.level === undefined ? "Level not observed" : `Level ${port.level.toFixed(2)}`}{port.calibrating ? " · Calibrating" : ""}</small></section>)}<section><small>Read-only. Physical jack mapping and calibration controls are not available through this connection.</small></section></div>
      : view === "capture" ? <div className="io-editor is-headphones"><section><span>CAPTURE OUT</span><small>Capture Out settings are not available through this connection. Use the Quad Cortex screen.</small></section></div>
      : <div className="io-editor is-headphones"><section><span>HP LEVEL</span><Dial label="Headphone level" value={io?.headphones?.level ?? 0} display={io?.headphones?.level === undefined ? "—" : io.headphones.level.toFixed(2)} min={0} max={1} disabled /></section><section><span>OBSERVED FEEDS</span>{io?.headphones?.feeds.map((feed) => <small key={feed.outputPortId}>{`Output port ${feed.outputPortId}: ${feed.level.toFixed(2)}`}</small>)}<small>Read-only. Adjust headphones on the Quad Cortex.</small></section><div className="io-headphone-meter"><span>LEVEL</span><small>—</small><QcLevelMeter /><QcLevelMeter /></div></div>}
  </section>;
}

/** Global EQ keeps the device's graph/tab/control composition; controls bind to the sparse semantic parameter list. */
export function CorOsGlobalEqScreen({ workflow, onBack }: { workflow: Workflow; onBack(): void }) {
  const { eq, loading, pending } = workflow;
  const [band, setBand] = useState(1);
  const [menu, setMenu] = useState(false);
  const [presetMenu, setPresetMenu] = useState(false);
  const [out, setOut] = useState(false);
  const unavailable = loading || pending || !eq;
  // The host's session fold, already limited to the Global EQ family with complete, unique identities.
  const presets = workflow.globalEq.status === "observed" ? workflow.globalEq.presets : [];
  const has = (index: number) => eq?.parameters.some((p) => p.parameterIndex === index) ?? false;
  const update = (offset: number, value: number) => void workflow.setEqBand(band, offset === 0 ? value : null, offset === 1 ? value : null, offset === 2 ? value : null, offset === 3 ? value : null, offset === 4 ? Boolean(value) : null);
  const indices = out ? [25, 26, 27] : [((band - 1) * 5), ((band - 1) * 5) + 1, ((band - 1) * 5) + 2];
  const gain = eqValue(eq, indices[0]); const frequency = eqValue(eq, indices[1]); const q = eqValue(eq, indices[2]);
  const filter = Math.round(eqValue(eq, (band - 1) * 5 + 3, 4));
  const graphNodes = [0, 1, 2, 3, 4].map((index) => ({
    x: [104, 210, 448, 607, 671][index] + (eqValue(eq, index * 5 + 1, 0.5) - 0.5) * 80,
    y: 125 - (eqValue(eq, index * 5, 0.5) - 0.5) * 80
  }));
  const response = `M32 125 ${graphNodes.map((node) => `L${node.x} ${node.y}`).join(" ")} L800 125`;
  return <section className={`qc-screen coros-global-eq${eq?.bypassed ? " is-off" : ""}${menu ? " is-filter-type-open" : ""}`} aria-label="Global EQ">
    <header><button className="global-eq-more" type="button" aria-expanded={presetMenu} onClick={() => setPresetMenu(!presetMenu)}><QcPresetStackIcon /></button><span><small>GLOBAL EQ</small><strong>Parametric-5</strong></span>
      <button type="button" className={`global-eq-power${eq?.bypassed ? " is-off" : ""}`} disabled={unavailable || eq?.bypassed === undefined} onClick={() => void workflow.setEqBypassed(!eq?.bypassed)}><i />{eq?.bypassed ? "OFF" : "ON"}</button>
      <button type="button" aria-label="Close Global EQ" data-qc-screen-navigation onClick={onBack}><QcEditorIcon kind="confirm" /></button></header>
    <div className="global-eq-graph"><div>{[46,79,105,125,143,158,172,184,263,343,369,389,407,422,436,448,527,574,607,633,653,671,686,700,712,791].map((x) => <i key={x} style={{ left: `${x / 8}%` }} />)}</div>
      <svg viewBox="0 0 800 252" preserveAspectRatio="none"><g className="eq-axis-labels"><text x="208" y="13">100</text><text x="471" y="13">1k</text><text x="736" y="13">10k</text></g><path d={response} /><g>{graphNodes.map((node, index) => <g key={index}><circle className={index === band - 1 ? "is-active" : ""} cx={node.x} cy={node.y} /><text x={node.x} y={node.y + 5}>{index + 1}</text></g>)}</g></svg>
    </div>
    {presetMenu && <><button type="button" className="eq-filter-type-scrim" aria-label="Close Global EQ preset menu" onClick={() => setPresetMenu(false)} /><div className="global-eq-preset-selector" role="menu" aria-label="Global EQ presets">
      <strong>Global EQ Presets</strong>
      {presets.length ? presets.map((preset) => <button type="button" role="menuitem" key={`${preset.id}-${preset.modelHash ?? 0}`} onClick={() => { setPresetMenu(false); void workflow.loadEqPreset(preset); }}>{preset.name || "Unnamed preset"}{preset.isDefault ? <small>Default</small> : null}</button>) : <p>{workflow.globalEq.status === "observed" ? "No Global EQ presets observed in this session." : globalEqUnavailableNotice(workflow.globalEq)}</p>}
    </div></>}
    <div className="global-eq-tabs">{[1,2,3,4,5].map((n) => <button type="button" key={n} className={!out && n === band ? "is-active" : ""} onClick={() => { setBand(n); setOut(false); }}>{n}</button>)}<button type="button" className={out ? "is-active" : ""} onClick={() => setOut(true)}>OUT</button></div>
    <div className="global-eq-controls">{out ? <>
      <section><span>OUT LEVEL</span><Dial label="Global EQ output" value={eqValue(eq,25) * 40 - 20} display={`${(eqValue(eq,25) * 40 - 20).toFixed(1)} dB`} min={-20} max={20} disabled={unavailable || !has(25)} onChange={(v) => void workflow.setEqOutput(v, null, null)} /></section>
      <section><span>OUT 1/2</span><PillSwitch label="OUT 1/2" value={eqValue(eq,26) >= .5} disabled={unavailable || !has(26)} onChange={(v) => void workflow.setEqOutput(null, v, null)} /></section>
      <section><span>OUT 3/4</span><PillSwitch label="OUT 3/4" value={eqValue(eq,27) >= .5} disabled={unavailable || !has(27)} onChange={(v) => void workflow.setEqOutput(null, null, v)} /></section>
    </> : <>
      <section><span>TYPE</span><button type="button" disabled={unavailable || !has((band-1)*5+3)} onClick={() => setMenu(!menu)}><QcEqIcon kind={filter === 0 ? "peak" : filter === 1 ? "high-pass" : filter === 2 ? "lo-pass" : filter === 3 ? "hi-shelf" : "lo-shelf"} />{EQ_FILTER_TYPE_LABELS[filter]?.toUpperCase() ?? "TYPE"}<QcUiIcon kind="down" /></button></section>
      <section><span>GAIN</span><Dial label={`Band ${band} gain`} value={gain} display={`${((gain - 0.5) * 24).toFixed(1)} dB`} min={0} max={1} disabled={unavailable || !has(indices[0])} onChange={(v) => update(0,v)} /></section>
      <section><span>FREQ</span><Dial label={`Band ${band} frequency`} value={frequency} display={formatGlobalEqFrequency(frequency)} min={0} max={1} disabled={unavailable || !has(indices[1])} onChange={(v) => update(1,v)} /></section>
      <section><span>Q</span><Dial label={`Band ${band} Q`} value={q} display={globalEqQ(q).toFixed(2)} min={0} max={1} disabled={unavailable || !has(indices[2])} onChange={(v) => update(2,v)} /></section>
      <section className="global-eq-bypass"><span>BYPASS {band}</span><button type="button" aria-pressed={eqValue(eq,(band-1)*5+4) < .5} disabled={unavailable || !has((band-1)*5+4)} onClick={() => update(4, eqValue(eq,(band-1)*5+4) >= .5 ? 0 : 1)}><QcUiIcon kind="power-toggle" /></button></section>
    </>}
    </div>
    {menu && <><button type="button" className="eq-filter-type-scrim" aria-label="Close filter type selector" onClick={() => setMenu(false)} /> <div className="eq-filter-type-selector" role="menu" aria-label={`Band ${band} filter type`}>{EQ_FILTER_TYPE_LABELS.map((label,index) => <button type="button" key={label} role="menuitemradio" aria-checked={filter === index} className={filter === index ? "is-selected" : ""} onClick={() => { update(3,index); setMenu(false); }}><i><QcEqIcon kind={(["peak","high-pass","lo-pass","hi-shelf","lo-shelf"] as const)[index]} /></i><span>{label}</span></button>)}</div></>}
  </section>;
}
