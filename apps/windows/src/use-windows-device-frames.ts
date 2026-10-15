import { useEffect, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import type { NativeStateFrames, PresetSnapshot } from "@qc-remote/client";
import type { QcStateUpdate } from "@qc-remote/core";
import { consumeQcNativeStateFrame } from "@qc-remote/ui";
import { listenTauri, TAURI_EVENTS } from "./tauri-transport";

type NativeFrame = NativeStateFrames<QcStateUpdate>["frames"][number] & {deviceId?: string};

export interface WindowsDeviceFrameSession {
  enabled: boolean;
  selectedDeviceId: MutableRefObject<string | undefined>;
  sequence: MutableRefObject<number>;
  streamId: MutableRefObject<string | undefined>;
  recoveryRevision: MutableRefObject<number>;
  available: MutableRefObject<boolean>;
  consume(states: readonly QcStateUpdate[], observedAt?: number, frame?: {sequence: number; streamId?: string}): unknown;
  setSnapshot: Dispatch<SetStateAction<PresetSnapshot>>;
  onStreamRestart(reason: "initial" | "restarted" | "gap"): void;
}

/** Windows-only subscription glue; all state reduction remains in shared UI/core. */
export function useWindowsDeviceFrames({
  enabled, selectedDeviceId, sequence, streamId, recoveryRevision, available, consume, setSnapshot, onStreamRestart
}: WindowsDeviceFrameSession) {
  useEffect(() => {
    if (!enabled) {
      setSnapshot((current) => current.tempoPulseEpochMs === undefined
        ? current
        : { ...current, tempoPulseEpochMs: undefined });
      return;
    }
    let disposed = false;
    let detach: (() => void) | undefined;
    void listenTauri<NativeFrame>(TAURI_EVENTS.stateFrame, ({ payload: frame }) => {
      if (disposed || (selectedDeviceId.current && frame.deviceId !== selectedDeviceId.current)) return;
      // The tempo pulse must share the renderer's monotonic clock. The broker
      // timestamp is useful for ordering, but includes an unmeasured hop from
      // USB receive to the WebView. Stamp the native tempo-clock event at arrival
      // so its animation can be anchored to the beat message we received.
      const receivedAt = frame.tempoClock
        ? performance.timeOrigin + performance.now()
        : frame.observedAt;
      consumeQcNativeStateFrame({ ...frame, observedAt: receivedAt }, { sequence, streamId, recoveryRevision, available, consume, setSnapshot, onStreamRestart });
    }).then((unlisten) => {
      if (disposed) unlisten();
      else detach = unlisten;
    }).catch(() => { if (!disposed) available.current = false; });
    return () => {
      disposed = true;
      detach?.();
    };
  }, [available, consume, enabled, selectedDeviceId, onStreamRestart, sequence, streamId, recoveryRevision, setSnapshot]);
}
