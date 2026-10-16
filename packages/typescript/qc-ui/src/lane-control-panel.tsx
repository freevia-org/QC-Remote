import { useState } from "react";
import type { BlockDetails, BlockParameter, LaneControl } from "@qc-remote/client";
import { sceneLetter } from "@qc-remote/core";
import { laneControlContextMenuItems, type ParameterEditorContextAction } from "./parameter-menu";
import { parameterDisplay } from "./parameter-model";
import { QcLevelMeter } from "./qc-level-meter";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QC_COLORS } from "@qc-remote/theme";
import { QcSceneSelector } from "./qc-shared-controls";
import { QcEditorIcon, QcUiIcon } from "./theme-icons";
import "./official-lane-controls.css";

/**
 * Input Gate and Lane Output: real per-row controls the device exposes
 * outside `BlockDetails.parameters` (they are not addressable Grid blocks),
 * reached by `laneControlDetails(row, control)`/`setLaneControlExpression`
 * instead. This panel reads live `BlockDetails` the same way
 * `CorOsParameterEditor` does, but is its own component rather than a branch
 * of that one - the same relationship `CorOsExpressionAssignment` already has
 * to it: a sibling overlay, not a mode of the generic editor.
 *
 * Live parameter editing (turning NOISE REDUCTION/INPUT GAIN, muting BYPASS)
 * is out of scope here - this panel only displays the block's current values
 * and opens expression assignment for it, which is the scope of this
 * feature. `setLaneControlParameter` is unused by this panel for exactly that
 * reason, not because it was forgotten.
 *
 * Lane Output's chrome matches how model 23000 "LaneOutputControl" actually
 * renders on device - see the `is-lane-output` branch below and
 * official-lane-controls.css. Its OUT LEVEL section has no data behind it and
 * is drawn at silence - see `LaneOutputLevelMeter`.
 */
export interface LaneControlPanelState {
  details: BlockDetails;
  control: LaneControl;
  activeScene: number;
  scenes: string[];
  pending: boolean;
  onSceneSelect(scene: number): void;
  onContextAction(action: ParameterEditorContextAction): void;
  onClose(): void;
}

function byName(details: BlockDetails, name: string): BlockParameter | undefined {
  return details.parameters.find((parameter) => parameter.name.toLowerCase() === name.toLowerCase());
}

/** The single flag that dims a field's label, value and knob together -
 * ParameterControl's own `controlDisabled` (parameter-editor.tsx), reused
 * here rather than reinvented. A disabled field's label, value and knob arc
 * share one dim ink and an enabled field's share one full-white ink, so this
 * has to be one flag rather than the label's own fixed style. */
function laneControlDisabled(parameter?: BlockParameter, gateBypassed = false): boolean {
  return !parameter || parameter.writable === false || parameter.enabled === false || gateBypassed;
}

/** Input Gate's BYPASS is the only "enable" the gate's own parameters carry:
 * the catalogue declares no `enableWhenOn`/`enableWhenOff` wiring for model
 * 28000, and the host reports `enabled: true` for every field of it (all 96
 * lane-control readbacks recorded from a physical unit), so
 * the per-field flags can never express the dimming the unit draws. The unit
 * dims NOISE REDUCTION and GAIN REDUCTION while the gate is bypassed and
 * leaves INPUT GAIN lit - the one control that still acts on the signal
 * through a bypassed gate - so the panel derives that from BYPASS itself. */
function inputGateBypassed(details: BlockDetails): boolean {
  return (byName(details, "BYPASS")?.normalizedValue ?? 0) >= .5;
}

/** A GAIN REDUCTION reading, normalised 0..1 along its -40..0 dB scale.
 *
 * `grMeter` is a live level, not a stored setting, and the host's only source
 * for it is the preset record, which holds the meter's zero-initialised slot:
 * every recorded readback is normalizedValue 0 (-40.0 dB, "full reduction")
 * with no live update behind it, while the unit at rest shows 0.0 dB with an
 * empty track. A reading of exactly the scale floor is therefore the absence
 * of a reading, and the panel draws the meter at rest (0 dB, no reduction)
 * as the unit does, rather than a full bar the unit never shows with the gate
 * idle. A real reduction (anything above the floor) passes through untouched. */
export function gainReductionReading(parameter?: BlockParameter): number {
  const value = parameter?.normalizedValue;
  return value == null || value <= .000001 ? 1 : value;
}

/** A live-valued readout for a real, writable lane-control parameter - the
 * same rotary dial `ParameterControl` draws elsewhere, but read-only here.
 *
 * Every other caller of QcRotaryDial passes its own `accent` (ParameterControl,
 * the EQ band controls, the IR loader channels); this one passes
 * `reference.primaryText` explicitly rather than falling back to
 * QcRotaryDial's own default accent, since neither Input Gate knob is
 * colourised - both read as a plain white fill. */
function LaneControlReadout({ label, parameter, knobClass, gateBypassed = false }: { label: string; parameter?: BlockParameter; knobClass?: string; gateBypassed?: boolean }) {
  const value = parameter?.normalizedValue ?? 0;
  const disabled = laneControlDisabled(parameter, gateBypassed);
  return <section className={disabled ? "is-disabled" : undefined}>
    <span>{label}</span>
    <QcRotaryDial className={`lane-control-knob${knobClass ? ` ${knobClass}` : ""}${disabled ? " is-unlit" : ""}`} value={value} accent={QC_COLORS.reference.primaryText} />
    <strong>{parameter ? parameterDisplay(parameter, value) : "—"}</strong>
    {parameter?.expression != null && parameter.expression > 0 && <span className="lane-control-expression" title={`Assigned to Expression Pedal ${parameter.expression}`}><QcEditorIcon kind="assignment-expression" /><b>EXP {parameter.expression}</b></span>}
  </section>;
}

/** GAIN REDUCTION is `type: "grMeter"`, `writable: false` - a read-only
 * meter, matching `fixture-input-gate`'s own `.input-gate-meter` (no dial,
 * no assign affordance offered anywhere for it).
 *
 * At rest (0.0 dB, the top of its -40..0 range - no reduction currently
 * happening) the track is uniformly empty. The meter runs the other way
 * round from a knob: fill is how far the reading has dropped from 0 dB, not
 * how far the raw value sits along its scale. */
function LaneControlMeter({ label, parameter, gateBypassed }: { label: string; parameter?: BlockParameter; gateBypassed: boolean }) {
  const value = gainReductionReading(parameter);
  const fill = 1 - value;
  /* A meter is never writable by design, so only a missing parameter, an
     explicit enabled:false or a bypassed gate dims it - not `writable`. */
  const dimmed = !parameter || parameter.enabled === false || gateBypassed;
  return <section className={dimmed ? "is-disabled" : undefined}>
    <span>{label}</span>
    <strong>{parameter ? parameterDisplay(parameter, value) : "—"}</strong>
    {/* The unit draws this as the shared 137 x 16 level meter: a track cut by the zero tick, unlit at rest. */}
    <QcLevelMeter className="lane-control-meter" value={fill} />
  </section>;
}

/** MUTE and SOLO are `type: "switch"` (boolean, 2 steps) - structurally
 * different from VOLUME/PAN's `type: "float"` - and the unit draws them as
 * flat rounded buttons, not the rotary dial `LaneControlReadout` draws for a
 * real float parameter: a `var(--qc-screen-surface-base)` box with the label
 * itself as the button face, no separate caption above it the way VOLUME/PAN
 * get one. MUTE uses `--qc-palette-ff2421`, SOLO uses `--qc-palette-0875e7` -
 * both already-declared tokens, not new literals.
 *
 * Neither field's engaged appearance is confirmed, so this does not invent a
 * lit/filled look for that case - `is-engaged` is wired from the real
 * `parameter.normalizedValue` so a future session can style it once that is
 * known, but today it renders identically either way. */
function LaneControlToggle({ label, parameter, tone }: { label: string; parameter?: BlockParameter; tone: "mute" | "solo" }) {
  const engaged = (parameter?.normalizedValue ?? 0) >= .5;
  return <section>
    <button
      type="button"
      className={`lane-control-toggle ${tone}${engaged ? " is-engaged" : ""}${parameter ? "" : " is-unlit"}`}
      disabled
      aria-label={`${label} is not editable from this panel`}
    >{label}</button>
    {parameter?.expression != null && parameter.expression > 0 && <span className="lane-control-expression" title={`Assigned to Expression Pedal ${parameter.expression}`}><QcEditorIcon kind="assignment-expression" /><b>EXP {parameter.expression}</b></span>}
  </section>;
}

/** OUT LEVEL: the unit's two-channel output meter, the fifth Lane Output field.
 *
 * Model 23000's catalogue entry and every recorded lane-control readback
 * carry VOLUME/PAN/MUTE/SOLO only - no OUT LEVEL parameter, and the preset
 * record's one undocumented extra value is a single number, not a left/right
 * pair - so there is no value to read. Keep both channel values unavailable,
 * with empty decorative tracks, rather than inventing a silence measurement.
 * The section is read-only and marks itself
 * `data-qc-live-meter="unavailable"` so nothing mistakes it for a reading.
 * Replace the placeholders with the live level once a protocol path carries it
 * (needs a USB capture of the meter's updates while audio plays). */
function LaneOutputLevelMeter() {
  return <section className="lane-control-outlevel" data-qc-live-meter="unavailable" aria-label="Out level, no live reading available">
    <span>OUT LEVEL</span>
    <div className="lane-control-level-readings"><b>—</b><b>—</b></div>
    <QcLevelMeter className="lane-control-level-track" />
    <QcLevelMeter className="lane-control-level-track" />
  </section>;
}

export function CorOsLaneControlPanel({ state }: { state: LaneControlPanelState }) {
  const { details, control, activeScene, scenes, pending, onSceneSelect, onContextAction, onClose } = state;
  const [menuOpen, setMenuOpen] = useState(false);
  const isInputGate = control === "inputGate";
  const bypassed = isInputGate && inputGateBypassed(details);
  const sceneCount = Math.max(1, scenes.length);
  const contextItems = laneControlContextMenuItems();
  const runContextAction = (action: ParameterEditorContextAction) => { setMenuOpen(false); onContextAction(action); };
  return <section className={`lane-control-panel${isInputGate ? " is-input-gate" : " is-lane-output"}`} aria-label={`${isInputGate ? "Input Gate Control" : "Lane Output Control"} panel`}>
    <header>
      <button className="lane-control-more" aria-label="Lane control contextual menu" aria-expanded={menuOpen} onClick={() => setMenuOpen((open) => !open)}><QcUiIcon kind="more" /></button>
      <span>
        <small>{isInputGate ? "INPUT GATE CONTROL" : "LANE OUTPUT CONTROL"}</small>
        <strong>Path {details.row + 1}</strong>
      </span>
      {/* The same shared shuttle parameter-editor.tsx and tempo-screen.tsx
          already use for this exact control, not a second hand-rolled
          <nav>/<b> - see official-lane-controls.css's own comment for what
          that duplication cost visually (a second background chip behind
          each arrow, ghosted directly on the pill on the real device). */}
      <QcSceneSelector
        className="lane-control-scene"
        letter={sceneLetter(activeScene)}
        onPrevious={() => onSceneSelect((activeScene - 1 + sceneCount) % sceneCount)}
        onNext={() => onSceneSelect((activeScene + 1) % sceneCount)}
      />
      <em />
      {/*
       * Lane Output Control's header has no power/bypass button at all -
       * only the context menu, title, scene selector and close button. Input
       * Gate Control's header does show one, so this stays conditional
       * rather than becoming unconditionally absent.
       */}
      {/* The unit paints this button the same bright green power symbol
          with the gate bypassed (the only state any capture holds: input-gate-control.png
          and a 2026-10 side-by-side of the real unit with BYPASS = 1) - it
          does not swap to the barred ring a bypassed block's editor header
          uses, and it is not dimmed. The bypass is carried by the dimmed
          NOISE REDUCTION / GAIN REDUCTION fields beside it, so `is-bypassed`
          stays on the element as a state hook only. What the button looks
          like with the gate enabled has never been captured. */}
      {isInputGate && <button
        className={`lane-control-power${bypassed ? " is-bypassed" : ""}`}
        disabled
        aria-label="Input Gate bypass is not editable from this panel"
      ><QcEditorIcon kind="bypass" /></button>}
      {/* Pinned to the header's last grid track explicitly: when Lane Output
          omits the power button above, plain DOM-order auto-placement would
          slide this button one track left into the now-empty power-button
          cell instead of leaving that cell blank - the unit keeps the close
          button flush at the panel's right edge with the vacated cell before
          it, not shifted inward. */}
      <button className="lane-control-close" style={{ gridColumn: 6 }} aria-label="Close panel" onClick={onClose}><QcUiIcon kind="check" /></button>
      {menuOpen && <div className="qc-block-context-menu qc-scroll-rail" role="menu" aria-label="Lane control contextual menu">
        {contextItems.map((item) => <button key={item.action} role="menuitem" disabled={item.disabled || pending} onClick={() => runContextAction(item.action)}><span><QcEditorIcon kind={item.icon} /></span>{item.label}</button>)}
      </div>}
    </header>
    <main>
      {isInputGate
        ? <>
          <LaneControlReadout label="NOISE REDUCTION" parameter={byName(details, "NOISE REDUCTION")} knobClass="reduction" gateBypassed={bypassed} />
          <LaneControlMeter label="GAIN REDUCTION" parameter={byName(details, "GAIN REDUCTION")} gateBypassed={bypassed} />
          <LaneControlReadout label="INPUT GAIN" parameter={byName(details, "INPUT GAIN")} knobClass="gain" />
        </>
        : <>
          {/*
           * VOLUME and PAN are real `type: "float"` parameters and render as
           * knobs via `LaneControlReadout`. MUTE and SOLO are real
           * `type: "switch"` booleans and render as `LaneControlToggle`
           * instead - see that component's own comment for the colours and
           * shape. OUT LEVEL, the fifth field, has no parameter behind it;
           * `LaneOutputLevelMeter` draws its structure at silence and says
           * why. The five fields are equal-width columns with a divider
           * between each, which `.lane-control-panel > main > section`'s
           * `border-right` already paints.
           */}
          <LaneControlReadout label="VOLUME" parameter={byName(details, "VOLUME")} />
          <LaneControlReadout label="PAN" parameter={byName(details, "PAN")} />
          <LaneControlToggle label="MUTE" parameter={byName(details, "MUTE")} tone="mute" />
          <LaneControlToggle label="SOLO" parameter={byName(details, "SOLO")} tone="solo" />
          <LaneOutputLevelMeter />
        </>}
    </main>
  </section>;
}
