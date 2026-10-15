import { useEffect, useMemo, useRef } from "react";
import type { KeyboardEvent, PointerEvent, WheelEvent } from "react";

/**
 * The one pointer/wheel/keyboard gesture layer behind every rotary control
 * `QcRotaryDial` already draws the dial once for every
 * screen; this is the matching single implementation of how it is turned:
 *
 * - a vertical drag, captured to the pointer that started it, turns the dial
 *   one step per `pixelsPerStep` pixels travelled, carrying the remainder;
 * - a wheel notch turns it one step, and settles `wheelSettleMs` after the
 *   last notch (no settle when the control has nothing to commit);
 * - ArrowUp/ArrowRight and ArrowDown/ArrowLeft nudge one step, and releasing
 *   the arrow settles;
 * - releasing *or cancelling* the drag settles on the value it reached, so a
 *   cancelled gesture can never leave a screen's draft frozen on an unsent
 *   value;
 * - a pending wheel settle never fires after the control unmounts, so a
 *   screen closed inside that window cannot write from beyond its lifetime.
 *
 * Per-screen constants are parameters, not copies: the block editor's knobs
 * and the splitter/mixer knobs turn one step per 7 px and settle 55 ms after
 * the wheel; the Tempo screen keeps its own 4 px and 200 ms as shipped (they
 * are not measured against the unit); the chassis encoders and master volume
 * use 6 px under a finger and 12 px under a mouse and commit every step.
 */

/** The shared ArrowUp/Right = +1, ArrowDown/Left = -1 keyboard nudge. */
export function rotaryKeyDirection(key: string): 1 | -1 | undefined {
  if (key === "ArrowUp" || key === "ArrowRight") return 1;
  if (key === "ArrowDown" || key === "ArrowLeft") return -1;
  return undefined;
}

/** Pixels per step for the block editor's knobs and faders and the splitter/mixer knobs. */
export const PARAMETER_ROTARY_PIXELS_PER_STEP = 7;
/** Wheel-settle debounce for the block editor's knobs and faders and the splitter/mixer knobs. */
export const PARAMETER_ROTARY_WHEEL_SETTLE_MS = 55;
/** The chassis encoders and master volume: 6 px per step under a finger, 12 px under a mouse or pen. */
export const hardwareRotaryPixelsPerStep = (pointerType: string) => pointerType === "touch" ? 6 : 12;

/** What turned the control. */
export type RotarySource = "drag" | "wheel" | "key";

export interface RotaryGestureOptions<E extends Element> {
  /** Drag distance per step, fixed or chosen from the pointer type at pointer-down. */
  pixelsPerStep: number | ((pointerType: string) => number);
  /** Settle this long after the last wheel notch. Omit when the control commits every step. */
  wheelSettleMs?: number;
  /** Refuse to start a drag, wheel turn or key nudge. A drag already under way still ends normally. */
  disabled?: boolean;
  /** Prevent the browser's default for cancelable pointer-down and pointer-move (no scroll or text selection under a chassis control). */
  preventPointerDefault?: boolean;
  /** Turn the control by whole steps; positive is clockwise. */
  onRotate(steps: number, source: RotarySource): void;
  /** The gesture ended: commit the value it reached. */
  onSettle?(): void;
  /** A drag began on this control. */
  onPointerStart?(event: PointerEvent<E>): void;
  /** The drag that began on this control ended, released or cancelled. Runs before `onSettle`. */
  onPointerEnd?(event: PointerEvent<E>, cancelled: boolean): void;
}

export interface RotaryGestureHandlers<E extends Element> {
  onPointerDown(event: PointerEvent<E>): void;
  onPointerMove(event: PointerEvent<E>): void;
  onPointerUp(event: PointerEvent<E>): void;
  onPointerCancel(event: PointerEvent<E>): void;
  onWheel(event: WheelEvent<E>): void;
  onKeyDown(event: KeyboardEvent<E>): void;
  onKeyUp(event: KeyboardEvent<E>): void;
  /** Retire a held arrow key when focus leaves before keyup. */
  onBlur(): void;
}

export function useRotaryGesture<E extends Element = HTMLButtonElement>(options: RotaryGestureOptions<E>): RotaryGestureHandlers<E> {
  // Handlers are stable; they read the latest options through this ref.
  const latest = useRef(options);
  latest.current = options;
  const drag = useRef<{ pointerId: number; y: number; pixelsPerStep: number; target: E } | undefined>(undefined);
  const mounted = useRef(true);
  const mountEpoch = useRef(0);
  const keys = useRef(new Set<string>());
  const wheelTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      mountEpoch.current++;
      if (wheelTimer.current !== undefined) clearTimeout(wheelTimer.current);
      wheelTimer.current = undefined;
      keys.current.clear();
      const held = drag.current;
      drag.current = undefined;
      if (held?.target.hasPointerCapture?.(held.pointerId)) held.target.releasePointerCapture(held.pointerId);
      // Retirement is not a release into whichever device replaced this control.
    };
  }, []);
  return useMemo(() => {
    const settle = () => {
      if (!mounted.current) return;
      if (wheelTimer.current !== undefined) clearTimeout(wheelTimer.current);
      wheelTimer.current = undefined;
      latest.current.onSettle?.();
    };
    const end = (event: PointerEvent<E>, cancelled: boolean) => {
      const gesture = drag.current;
      if (!mounted.current || !gesture || gesture.pointerId !== event.pointerId) return;
      drag.current = undefined;
      if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      latest.current.onPointerEnd?.(event, cancelled);
      settle();
    };
    return {
      onPointerDown(event) {
        const current = latest.current;
        if (!mounted.current || current.disabled) return;
        const previous = drag.current;
        if (previous?.target.hasPointerCapture?.(previous.pointerId)) previous.target.releasePointerCapture(previous.pointerId);
        if (current.preventPointerDefault && event.cancelable) event.preventDefault();
        event.currentTarget.setPointerCapture?.(event.pointerId);
        const pixelsPerStep = typeof current.pixelsPerStep === "function" ? current.pixelsPerStep(event.pointerType) : current.pixelsPerStep;
        drag.current = { pointerId: event.pointerId, y: event.clientY, pixelsPerStep, target: event.currentTarget };
        current.onPointerStart?.(event);
      },
      onPointerMove(event) {
        const gesture = drag.current;
        if (!mounted.current || !gesture || gesture.pointerId !== event.pointerId) return;
        if (latest.current.preventPointerDefault && event.cancelable) event.preventDefault();
        const steps = Math.trunc((gesture.y - event.clientY) / gesture.pixelsPerStep);
        if (!steps) return;
        gesture.y -= steps * gesture.pixelsPerStep;
        latest.current.onRotate(steps, "drag");
      },
      onPointerUp(event) { end(event, false); },
      onPointerCancel(event) { end(event, true); },
      onWheel(event) {
        const current = latest.current;
        if (!mounted.current || current.disabled) return;
        event.preventDefault();
        current.onRotate(event.deltaY < 0 ? 1 : -1, "wheel");
        if (current.wheelSettleMs === undefined) return;
        if (wheelTimer.current !== undefined) clearTimeout(wheelTimer.current);
        const epoch = mountEpoch.current;
        wheelTimer.current = setTimeout(() => { if (mounted.current && epoch === mountEpoch.current) settle(); }, current.wheelSettleMs);
      },
      onKeyDown(event) {
        const direction = rotaryKeyDirection(event.key);
        if (!mounted.current || latest.current.disabled || direction === undefined) return;
        if (event.repeat && !keys.current.has(event.key)) return;
        keys.current.add(event.key);
        event.preventDefault();
        latest.current.onRotate(direction, "key");
      },
      onKeyUp(event) {
        if (mounted.current && rotaryKeyDirection(event.key) !== undefined && keys.current.delete(event.key)) settle();
      },
      onBlur() {
        if (!mounted.current || keys.current.size === 0) return;
        keys.current.clear();
        settle();
      }
    } satisfies RotaryGestureHandlers<E>;
  }, []);
}

export interface RotaryValueOptions<E extends Element> extends Omit<RotaryGestureOptions<E>, "onRotate" | "onSettle"> {
  /** The value the control currently shows. */
  value: number;
  /** One step, in the control's own units. */
  step: number;
  /** Snap and clamp a candidate value onto the control's range. */
  quantize(next: number): number;
  /** The draft value moved; the screen shows it until the commit lands. */
  onDraftChange(value: number): void;
  /** The gesture ended on this value. */
  onCommit(value: number): void;
}

/**
 * A rotary control over a plain value: `useRotaryGesture` plus the value it
 * reached. `set` moves the draft from outside a gesture (a select or switch
 * sharing the control's draft).
 */
export function useRotaryValue<E extends Element = HTMLButtonElement>(options: RotaryValueOptions<E>): RotaryGestureHandlers<E> & { set(next: number): number } {
  const { value, step, quantize, onDraftChange, onCommit, ...gesture } = options;
  const latest = useRef(options);
  latest.current = options;
  const valueRef = useRef(value);
  useEffect(() => { valueRef.current = value; }, [value]);
  const set = useMemo(() => (next: number) => {
    const quantized = latest.current.quantize(next);
    valueRef.current = quantized;
    latest.current.onDraftChange(quantized);
    return quantized;
  }, []);
  const handlers = useRotaryGesture<E>({
    ...gesture,
    onRotate: (steps) => { if (!latest.current.disabled) set(valueRef.current + steps * latest.current.step); },
    onSettle: () => latest.current.onCommit(valueRef.current)
  });
  return useMemo(() => ({ ...handlers, set }), [handlers, set]);
}
