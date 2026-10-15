import type { KeyboardEvent, MouseEvent } from "react";
import { QcModeGlyph } from "./theme-icons";

/**
 * The two parts of the Brightness pane that the unit draws and that both the
 * fixture (coros-screen-fixtures.tsx) and the live Settings screen
 * (device-settings-screen.tsx) need, written once so they cannot drift:
 *
 *  - the instruction paragraph, whose first line carries an up triangle and a
 *    down triangle where the footswitch names UP and DOWN would read. The unit
 *    draws drawn marks there (corpus settings-system.png: "Turn the [up], [down],
 *    and TEMPO footswitches"), never the words.
 *  - the row of three mode marks at the bottom, which the Brightness pane draws
 *    and the live screen used to omit. Tapping a mark toggles whether that mode
 *    dims its LEDs; a faint mark is off.
 *
 * Nothing here is sized in px: every dimension is em or cqw so the pane scales
 * with the 800px canvas it sits in.
 */

export const QC_BRIGHTNESS_MODES = ["PRESET", "SCENE", "STOMP"] as const;
export type QcBrightnessMode = typeof QC_BRIGHTNESS_MODES[number];

/**
 * The state the unit's own captured frame shows (corpus settings-system.png):
 * PRESET and SCENE faint, STOMP lit. Used by noninteractive static references
 * only; live settings must report their own flags.
 */
export const QC_BRIGHTNESS_MODE_DEFAULTS: Readonly<Record<QcBrightnessMode, boolean>> = { PRESET: false, SCENE: false, STOMP: true };

const MODE_LABELS: Record<QcBrightnessMode, string> = { PRESET: "Preset", SCENE: "Scene", STOMP: "Stomp" };

/** A filled triangle the height of a capital letter, pointing up or down. Drawn with a clip, not a
 * vector path, so it is no embedded vector owner: official-settings.css holds its shape. */
export function QcFootswitchTriangle({ direction }: { direction: "up" | "down" }) {
  return <span className={`qc-footswitch-triangle is-${direction}`} aria-hidden="true" />;
}

/**
 * The paragraph. `lines` is the device's own three-line break with the first
 * line split where the marks go: [before the marks, after them, second line,
 * third line].
 */
export function QcBrightnessInstructions({ lines }: { lines: readonly [string, string, string, string] }) {
  return <p>
    {lines[0]} <QcFootswitchTriangle direction="up" />, <QcFootswitchTriangle direction="down" />{lines[1]}
    <br />
    {lines[2]}
    <br />
    {lines[3]}
  </p>;
}

export function QcBrightnessModeRow({ dimmed, disabled, onToggle }: {
  /** Observed live flags; only noninteractive reference calls use captured defaults. */
  dimmed?: Partial<Record<QcBrightnessMode, boolean | undefined>>;
  disabled?: boolean;
  /** Omit for the static fixture, which draws the marks without a control behind them. */
  onToggle?(mode: QcBrightnessMode, next: boolean): void;
}) {
  return <footer>
    {QC_BRIGHTNESS_MODES.map((mode) => {
      const on = onToggle ? dimmed?.[mode] : dimmed?.[mode] ?? QC_BRIGHTNESS_MODE_DEFAULTS[mode];
      return <button
        key={mode}
        type="button"
        className={on === undefined ? "is-unavailable" : on ? "is-lit" : "is-faint"}
        disabled={disabled || (onToggle !== undefined && on === undefined)}
        tabIndex={onToggle ? undefined : -1}
        aria-pressed={on}
        aria-label={`Dim LEDs in ${MODE_LABELS[mode]} mode`}
        title={on === undefined ? "Mode dimming has not been observed in this session." : undefined}
        onClick={() => { if (on !== undefined) onToggle?.(mode, !on); }}
      ><QcModeGlyph mode={mode} /></button>;
    })}
  </footer>;
}

/**
 * Brightness is changed on the unit with its footswitches, so its bars carry no
 * visible control. A bar is still a control here: this is an invisible hit
 * area laid over it, so a tap sets the value the tapped segment ends at and the
 * arrow keys step it. Segments count from the left, so a tap at fraction f of
 * the bar lights ceil(f * segments) of them.
 */
export function QcSegmentBarHit({ label, value, max, segments, disabled, onChange }: {
  label: string;
  value: number | undefined;
  max: number;
  segments: number;
  disabled?: boolean;
  onChange(next: number): void;
}) {
  const clamp = (next: number) => Math.max(0, Math.min(max, next));
  const tap = (event: MouseEvent<HTMLButtonElement>) => {
    // Keyboard activation has no pointer coordinate. Arrow keys own slider
    // changes; Enter/Space must not interpret their synthetic click as zero.
    if (disabled || value === undefined || event.detail === 0) return;
    const box = event.currentTarget.getBoundingClientRect();
    if (box.width <= 0) return;
    onChange(clamp(Math.ceil(((event.clientX - box.left) / box.width) * segments)));
  };
  const key = (event: KeyboardEvent<HTMLButtonElement>) => {
    const step = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0;
    if (disabled || !step || value === undefined) return;
    event.preventDefault();
    onChange(clamp(value + step));
  };
  return <button
    type="button"
    className="qc-segment-bar-hit"
    role="slider"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={max}
    aria-valuenow={value}
    disabled={disabled || value === undefined}
    onClick={tap}
    onKeyDown={key}
  />;
}
