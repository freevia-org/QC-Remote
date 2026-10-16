import type { CSSProperties, ReactNode } from "react";
import type { RotaryGestureHandlers } from "./use-rotary-gesture";

/**
 * The interactive shell around a rotary drawing (`QcRotaryDial`, or the cab
 * strip's `QcBarSlider`): one button that is an ARIA slider with its range and
 * value, driven by `useRotaryGesture`. It adds no class
 * and no element of its own, so each screen keeps its own class names and
 * stylesheet.
 */
export function QcRotaryControl({ className, style, disabled, label, value, min, max, valueText, gesture, children }: {
  className: string;
  style?: CSSProperties;
  disabled?: boolean;
  /** The accessible name; the value is announced separately through `valueText`. */
  label: string;
  value: number;
  min: number;
  max: number;
  valueText: string;
  gesture: RotaryGestureHandlers<HTMLButtonElement>;
  children: ReactNode;
}) {
  return <button
    type="button"
    className={className}
    style={style}
    disabled={disabled}
    role="slider"
    aria-label={label}
    aria-valuemin={min}
    aria-valuemax={max}
    aria-valuenow={value}
    aria-valuetext={valueText}
    onPointerDown={gesture.onPointerDown}
    onPointerMove={gesture.onPointerMove}
    onPointerUp={gesture.onPointerUp}
    onPointerCancel={gesture.onPointerCancel}
    onWheel={gesture.onWheel}
    onKeyDown={gesture.onKeyDown}
    onKeyUp={gesture.onKeyUp}
    onBlur={gesture.onBlur}
  >{children}</button>;
}
