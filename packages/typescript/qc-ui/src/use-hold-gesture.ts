import { useEffect, useMemo, useRef } from "react";

/**
 * How long a press must be held before it becomes a hold, in milliseconds:
 * the scene-assignment hold on a block-editor control, pinning a model in the
 * device browser, splitting a hybrid mode and an encoder footswitch's hold
 * action all use this one value.
 */
export const QC_HOLD_DELAY_MS = 650;

/** Where a press started or has moved to. Every field is optional so keyboard and synthetic presses can arm a hold too. */
export interface HoldPoint {
  pointerId?: number;
  clientX?: number;
  clientY?: number;
}

export interface HoldGestureOptions<T> {
  /** Defaults to `QC_HOLD_DELAY_MS`. */
  delayMs?: number;
  /** Movement beyond this many pixels from the press cancels the hold. Omit to ignore movement. */
  moveTolerancePx?: number;
  /** The press matured into a hold. Without it, `start` only resets the gesture. */
  onHold?(payload: T): void;
}

export interface HoldGesture<T> {
  /** Begin a press, replacing (and cancelling) any earlier one. */
  start(payload: T, point?: HoldPoint): void;
  /** Cancel the pending hold once the press has moved past the tolerance. */
  move(point: HoldPoint): void;
  /** End or abandon the press without a hold. A point from another pointer is ignored. */
  cancel(point?: HoldPoint): void;
  /** Whether the current press has already fired its hold. */
  fired(): boolean;
  /** Read and clear `fired`, for swallowing the click that follows a hold. */
  consumeFired(): boolean;
}

/**
 * The one press-and-hold timer. Every press
 * clears the previous timer before arming a new one, any end, cancel or move
 * past the tolerance clears it, and unmounting clears it, so a hold can never
 * fire for a press that has already ended or a control that is gone.
 */
export function useHoldGesture<T = void>(options: HoldGestureOptions<T> = {}): HoldGesture<T> {
  const latest = useRef(options);
  latest.current = options;
  const press = useRef<{ timer?: ReturnType<typeof setTimeout>; point?: HoldPoint; fired: boolean }>({ fired: false });
  useEffect(() => () => {
    if (press.current.timer !== undefined) clearTimeout(press.current.timer);
    press.current.timer = undefined;
  }, []);
  return useMemo(() => {
    const clear = () => {
      if (press.current.timer !== undefined) clearTimeout(press.current.timer);
      press.current.timer = undefined;
      press.current.point = undefined;
    };
    const samePointer = (point: HoldPoint | undefined) => point?.pointerId === undefined || press.current.point?.pointerId === undefined || point.pointerId === press.current.point.pointerId;
    return {
      start(payload, point) {
        clear();
        press.current.fired = false;
        press.current.point = { pointerId: point?.pointerId, clientX: point?.clientX, clientY: point?.clientY };
        if (!latest.current.onHold) return;
        press.current.timer = setTimeout(() => {
          press.current.timer = undefined;
          press.current.fired = true;
          latest.current.onHold?.(payload);
        }, latest.current.delayMs ?? QC_HOLD_DELAY_MS);
      },
      move(point) {
        const origin = press.current.point;
        const tolerance = latest.current.moveTolerancePx;
        if (!origin || tolerance === undefined || !samePointer(point)) return;
        if (origin.clientX === undefined || origin.clientY === undefined || point.clientX === undefined || point.clientY === undefined) return;
        if (Math.hypot(point.clientX - origin.clientX, point.clientY - origin.clientY) <= tolerance) return;
        clear();
      },
      cancel(point) {
        if (!samePointer(point)) return;
        clear();
      },
      fired: () => press.current.fired,
      consumeFired() {
        const fired = press.current.fired;
        press.current.fired = false;
        return fired;
      }
    } satisfies HoldGesture<T>;
  }, []);
}
