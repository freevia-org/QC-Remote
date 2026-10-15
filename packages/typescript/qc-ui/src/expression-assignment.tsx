import type { BlockDetails, BlockParameter, LaneControl } from "@qc-remote/client";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRotaryDial } from "./qc-rotary-dial";
import { QcExpressionAssignmentLink, QcUiIcon } from "./theme-icons";
import "./official-expression.css";

export type ExpressionAssignmentTarget = number | "bypass";

/**
 * What this screen is currently assigning: a Grid block's `(row, column)` -
 * reached through `blockDetails`/`setParameterExpression`/`setExpressionBypass`
 * - or a lane control's `(row, control)` - reached through
 * `laneControlDetails`/`setLaneControlExpression` only, with no bypass write of
 * its own at all. `details.row`/`details.column` still carry the block address
 * for the "block" case; a lane control's own row comes from `details.row` too,
 * its column being whatever virtual column the gateway reports for it (not a
 * real Grid position, and not used to address the lane-control write calls).
 */
export type ExpressionAssignmentAddress =
  | { kind: "block" }
  | { kind: "lane"; control: LaneControl };

/**
 * The live Expression 1/2 pedal assignment screen, driven by the block's own
 * `BlockDetails` - `parameters[].expressionAssignable/expression` for a real
 * parameter and `bypassExpression` for the block's own mute.
 *
 * Assign, trim and unassign a real parameter, a lane control, or the block's
 * own bypass - pedal 0 clears any of the three, and the device acts on a
 * bypass clear the same way it does a parameter clear, so it is offered here
 * too.
 *
 * The bypass panel below (SWITCH ON/INVERT RANGE/SWITCH DELAY/LATCH EMULATION)
 * is not confirmed against the device screen and may not match its layout
 * exactly. Its own UNASSIGN affordance is added regardless: the underlying
 * clear behavior is confirmed independently of the panel's exact layout.
 */
export interface ExpressionAssignmentState {
  details: BlockDetails;
  /** Which wire address `details` was read from and every write here targets. */
  address: ExpressionAssignmentAddress;
  /** 1-indexed, matching the wire protocol's own Expression 1/Expression 2. */
  pedal: 1 | 2;
  onPedalChange(pedal: 1 | 2): void;
  /** Which tile's settings the left panel shows. */
  selected: ExpressionAssignmentTarget;
  onSelect(target: ExpressionAssignmentTarget): void;
  pending: boolean;
  /** Assign `target` to the active pedal at its current (or a default) range. */
  onAssign(target: ExpressionAssignmentTarget): void;
  onSetRange(index: number, minimum: number, maximum: number): void;
  /** Clear a parameter's, lane control's, or the block's own bypass expression assignment outright. */
  onUnassign(target: ExpressionAssignmentTarget): void;
  onClose(): void;
}

function isAssignedToActivePedal(details: BlockDetails, pedal: 1 | 2, target: ExpressionAssignmentTarget): boolean {
  if (target === "bypass") return details.bypassExpression?.pedal === pedal;
  const parameter = details.parameters.find((candidate) => candidate.index === target);
  return parameter?.expression === pedal;
}

function ExpressionTile({ label, assigned, pending, selected, onSelect }: { label: string; assigned: boolean; pending: boolean; selected: boolean; onSelect(): void }) {
  return <section className={`${assigned ? "is-assigned-tile" : ""}${selected ? " is-selected-tile" : ""}`.trim() || undefined}>
    <span>{label}<b><QcExpressionAssignmentLink assigned={assigned} /></b></span>
    <button className={assigned ? "is-assigned" : ""} disabled={pending} onClick={onSelect}>{assigned ? "ASSIGNED" : "ASSIGN"}</button>
  </section>;
}

/** Read-only state: absence is not evidence for either binary position. */
function ObservedBypassFlag({ value }: { value: boolean | undefined }) {
  return typeof value === "boolean" ? <label>
    <QcPillToggle index={value ? 0 : 1} />
    <b>On</b><small>Off</small>
  </label> : <b>Unavailable</b>;
}

/** Wire mode0 Stop /1 Switch /2 Heel-Toe; absence is not a selected role. */
function ObservedBypassMode({ value }: { value: number | undefined }) {
  return value === 0 || value === 1 || value === 2 ? <label>
    <QcPillToggle positions={3} index={2 - value} />
    <b>Heel-Toe</b>
    <small>Switch<br />Stop</small>
  </label> : <b>Unavailable</b>;
}

export function CorOsExpressionAssignment({ assignment }: { assignment: ExpressionAssignmentState }) {
  const { details, address, pedal, onPedalChange, selected, onSelect, pending, onAssign, onSetRange, onUnassign, onClose } = assignment;
  // GAIN REDUCTION on Input Gate is expressionAssignable: true, writable:
  // false (it is a read-only grMeter) - offering it an assign affordance
  // would draw a control that can never do anything, so both are required
  // together, not `expressionAssignable` alone.
  const assignable = details.parameters.filter((parameter) => parameter.expressionAssignable && parameter.writable);
  // A lane control has no `bypassExpression` of its own to assign at all - see
  // the module comment on `ExpressionAssignmentAddress` - so its tile grid is
  // every assignable parameter and nothing else.
  const tiles: Array<{ target: ExpressionAssignmentTarget; label: string }> = [
    ...(address.kind === "lane" ? [] : [{ target: "bypass" as const, label: "BYPASS" }]),
    ...assignable.map((parameter) => ({ target: parameter.index, label: parameter.name.toUpperCase() }))
  ];
  const selectedIsBypass = selected === "bypass";
  const selectedParameter = selectedIsBypass ? undefined : assignable.find((parameter) => parameter.index === selected);
  const selectedParameterAssigned = selectedParameter?.expression === pedal;
  const selectedBypassAssigned = selectedIsBypass && details.bypassExpression?.pedal === pedal;

  return <section className="qc-screen expression-bypass-official is-trim" aria-label="Expression pedal assignment">
    <header>
      <button aria-label="Close expression assignment" onClick={onClose}><QcUiIcon kind="close" /></button>
      <button aria-pressed={pedal === 1} onClick={() => onPedalChange(1)}>Expression 1</button>
      <button aria-pressed={pedal === 2} onClick={() => onPedalChange(2)}>Expression 2</button>
      <button aria-label="Done" onClick={onClose}><QcUiIcon kind="check" /></button>
    </header>
    <p>
      Please choose which parameters you wish to control.
      <br />
      You can assign multiple at once.
    </p>
    {selectedIsBypass
      ? <main className="expression-switch-panel">
        <section>
          <button aria-label="Bypass expression settings" disabled><QcUiIcon kind="power-toggle" /></button>
          {selectedBypassAssigned && <button className="expression-unassign" disabled={pending} onClick={() => onUnassign("bypass")}>UNASSIGN</button>}
        </section>
        <section>
          <span>SWITCH ON</span>
          <ObservedBypassMode value={details.bypassExpression?.mode} />
        </section>
        <section>
          <span>INVERT RANGE</span>
          <ObservedBypassFlag value={details.bypassExpression?.invert} />
        </section>
        <section className="switch-delay">
          <span>SWITCH DELAY</span>
          <b>—</b>
        </section>
        <section className="switch-latch">
          <span>LATCH EMULATION</span>
          <ObservedBypassFlag value={details.bypassExpression?.latchEmulation} />
        </section>
      </main>
      : <main className="expression-switch-panel is-trim">
        <section className="trim-hint">
          <span>Set the minimum and maximum the pedal reaches for this parameter.</span>
          <small>{selectedParameter?.name ?? "Select an assignable parameter"}</small>
          {selectedParameterAssigned && <button className="expression-unassign" disabled={pending} onClick={() => onUnassign(selectedParameter!.index)}>UNASSIGN</button>}
        </section>
        <section className="switch-delay">
          <span>MIN RANGE</span>
          <b>{selectedParameter ? `${Math.round((selectedParameter.expressionMinimum ?? 0) * 100)} %` : "—"}</b>
          <input type="range" min="0" max="1" step={.01} disabled={!selectedParameter || pending} value={selectedParameter?.expressionMinimum ?? 0}
            aria-label="Minimum range" onChange={(event) => selectedParameter && onSetRange(selectedParameter.index, Number(event.target.value), selectedParameter.expressionMaximum ?? 1)} />
          <QcRotaryDial className={`expression-rotary${selectedParameter ? "" : " is-unlit"}`} value={selectedParameter?.expressionMinimum ?? 0} />
        </section>
        <section className="switch-delay">
          <span>MAX RANGE</span>
          <b>{selectedParameter ? `${Math.round((selectedParameter.expressionMaximum ?? 1) * 100)} %` : "—"}</b>
          <input type="range" min="0" max="1" step={.01} disabled={!selectedParameter || pending} value={selectedParameter?.expressionMaximum ?? 1}
            aria-label="Maximum range" onChange={(event) => selectedParameter && onSetRange(selectedParameter.index, selectedParameter.expressionMinimum ?? 0, Number(event.target.value))} />
          <QcRotaryDial className="expression-rotary" value={selectedParameter?.expressionMaximum ?? 1} pointer="var(--qc-palette-6b716b)" />
        </section>
      </main>}
    <div className="expression-parameter-grid">
      {tiles.map(({ target, label }) => {
        const assigned = isAssignedToActivePedal(details, pedal, target);
        return <ExpressionTile key={String(target)} label={label} assigned={assigned} pending={pending} selected={selected === target}
          onSelect={() => { onSelect(target); if (!assigned) onAssign(target); }} />;
      })}
    </div>
  </section>;
}
