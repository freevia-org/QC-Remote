import { QC_MAXIMUM_TEMPO_BPM, QC_MINIMUM_TEMPO_BPM, requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useEffect, useRef } from "react";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { DeviceHistoryEntry } from "./use-device-history";
import type { QcController } from "./use-qc-controller";
import { canAdoptNativeState, SupersededPresetOperation } from "@qc-remote/core";
import { useScopedGateway } from "./scoped-gateway.ts";
import { drainLatestValue } from "./latest-value-writer.ts";

type TempoSource = "Encoder" | "Tap";

export interface ContinuousControlWorkflowOptions {
  controller: QcController;
  gateway: GatewayTransport;
  connected: boolean;
  demo: boolean;
  reconcile(snapshot: PresetSnapshot): void;
  recordHistory?(entry: DeviceHistoryEntry): void;
  notice(message: string): void;
  fail(error: unknown): void;
}

type TempoQueue = {
  noticeTimer?: number;
  running: boolean;
  expected?: number;
  original?: number;
  target?: number;
  source: TempoSource;
  lastAccepted?: number;
  lastDetail?: string;
  token?: ReturnType<QcController["beginTempo"]>;
};

type VolumeQueue = {
  running: boolean;
  /** Last value accepted by the device; the precondition for the next write. */
  confirmed?: number;
  /** Value written to USB and waiting for the QC's pushed type-17 echo. */
  awaiting?: number;
  /** Newest value not yet handed to the device writer. */
  target?: number;
  /** Value owned and displayed by the current local gesture. */
  desired?: number;
  gestureActive: boolean;
  explicit: boolean;
  releaseTimer?: number;
  confirmationTimer?: number;
  lastAccepted?: number;
  lastDetail?: string;
};

// Device writes start with the first encoder step. Footer completion follows
// the interaction boundary (pointer release, or the wheel/keyboard quiet
// boundary); delaying the write makes the physical QC and the knob feel
// detached from the gesture. The latest-value drain still coalesces newer
// targets that arrive while one USB write is in flight.
const CONTROL_COMPLETION_QUIET_MS = 300;
const INTERACTION_RELEASE_QUIET_MS = 120;
const VOLUME_CONFIRMATION_TIMEOUT_MS = 1_500;

/** Coalesced realtime encoders with one in-flight write and latest-value wins. */
export function useContinuousControlWorkflow(options: ContinuousControlWorkflowOptions) {
  const { controller, connected, demo } = options;
  // Hosts pass fresh callbacks on most renders. Read them through a ref so the
  // drains, timers and the echo subscription below keep their identity.
  const host = useRef(options);
  host.current = options;
  const notice = useCallback((message: string) => host.current.notice(message), []);
  const fail = useCallback((error: unknown) => host.current.fail(error), []);
  const recordHistory = useCallback((entry: DeviceHistoryEntry) => host.current.recordHistory?.(entry), []);
  // Depend on the controller's members, each a stable callback or ref, never
  // on the controller object: it is a new object whenever the snapshot moves,
  // and depending on it re-subscribed the Master Volume echo listener on
  // every native frame.
  const {
    lifetime, snapshotRef, acceptSnapshot, acceptInteractionObservation, beginInteraction, beginTempo,
    cancelInteraction, failCommand, releaseInteraction, settleCommand, subscribeInteractionObservation,
    updateInteraction, updateSnapshot
  } = controller;
  const { gateway, origin } = useScopedGateway(options.gateway, lifetime, controller.snapshot.setlistKey);
  const tempo = useRef<TempoQueue>({ running: false, source: "Encoder" });
  const volume = useRef<VolumeQueue>({ running: false, gestureActive: false, explicit: false });
  const tempoReleaseTimer = useRef<number | undefined>(undefined);

  const publishVolumeCompletion = useCallback(() => {
    const queue = volume.current;
    // A gesture is one user action even when the USB writer becomes idle
    // between pointer steps. Nothing reaches the footer until contact ends
    // and the final desired value has been accepted by the device.
    if (queue.gestureActive || queue.running || queue.awaiting !== undefined || queue.target !== undefined || queue.lastAccepted === undefined) return;
    notice(queue.lastDetail ?? `Master Volume set to ${queue.lastAccepted}.`);
    queue.lastAccepted = undefined;
    queue.lastDetail = undefined;
    queue.confirmed = undefined;
    queue.desired = undefined;
  }, [notice]);

  const releaseVolumeInteraction = useCallback(() => {
    const queue = volume.current;
    if (queue.releaseTimer !== undefined) window.clearTimeout(queue.releaseTimer);
    queue.releaseTimer = undefined;
    queue.explicit = false;
    queue.gestureActive = false;
    releaseInteraction({ kind: "master-volume" },
      queue.desired ?? snapshotRef.current.masterVolume);
    publishVolumeCompletion();
  }, [releaseInteraction, snapshotRef, publishVolumeCompletion]);

  const beginMasterVolumeInteraction = useCallback(() => {
    const queue = volume.current;
    if (queue.releaseTimer !== undefined) window.clearTimeout(queue.releaseTimer);
    queue.releaseTimer = undefined;
    queue.explicit = true;
    queue.gestureActive = true;
    if (queue.confirmed === undefined) queue.confirmed = snapshotRef.current.masterVolume;
    if (queue.desired === undefined) queue.desired = snapshotRef.current.masterVolume;
    beginInteraction({ kind: "master-volume" },
      queue.desired);
  }, [beginInteraction, snapshotRef]);

  const endMasterVolumeInteraction = useCallback(() => releaseVolumeInteraction(), [releaseVolumeInteraction]);

  const scheduleVolumeInteractionRelease = useCallback(() => {
    const queue = volume.current;
    if (queue.explicit) return;
    if (queue.releaseTimer !== undefined) window.clearTimeout(queue.releaseTimer);
    queue.releaseTimer = window.setTimeout(releaseVolumeInteraction, INTERACTION_RELEASE_QUIET_MS);
  }, [releaseVolumeInteraction]);

  const scheduleTempoInteractionRelease = useCallback((value: number) => {
    if (tempoReleaseTimer.current !== undefined) window.clearTimeout(tempoReleaseTimer.current);
    tempoReleaseTimer.current = window.setTimeout(() => {
      tempoReleaseTimer.current = undefined;
      releaseInteraction({ kind: "tempo" }, value);
    }, INTERACTION_RELEASE_QUIET_MS);
  }, [releaseInteraction]);

  const scheduleTempoNotice = useCallback(() => {
    const queue = tempo.current;
    if (queue.noticeTimer !== undefined) window.clearTimeout(queue.noticeTimer);
    const publish = () => {
      const current = tempo.current;
      current.noticeTimer = undefined;
      if (current.running || current.target !== undefined) {
        current.noticeTimer = window.setTimeout(publish, 50);
        return;
      }
      if (current.lastAccepted === undefined) return;
      notice(current.lastDetail ?? `${current.source} tempo set to ${current.lastAccepted} BPM.`);
      current.lastAccepted = undefined;
      current.lastDetail = undefined;
    };
    queue.noticeTimer = window.setTimeout(publish, CONTROL_COMPLETION_QUIET_MS);
  }, [notice]);

  const drainTempo = useCallback(async () => {
    const queue = tempo.current;
    await drainLatestValue(queue, {
      guard: (current) => current.expected,
      write: (target, expected) => gateway.setTempo(target, expected, snapshotRef.current.presetName, requirePresetIdentity(snapshotRef.current)),
      accept: (result, target, current) => {
        lifetime.assertCurrent(origin);
        current.expected = result.snapshot?.tempo ?? target;
        if (result.snapshot) acceptSnapshot(result.snapshot);
        current.lastAccepted = target;
        current.lastDetail = result.detail;
      },
      drained: (current) => {
        const original = current.original;
        const finalValue = current.expected;
        if (original !== undefined && finalValue !== undefined && original !== finalValue) {
          recordHistory({
            label: "tempo change",
            execute: (current) => gateway.setTempo(original, finalValue, current.presetName, requirePresetIdentity(current)),
            redo: (current) => gateway.setTempo(finalValue, original, current.presetName, requirePresetIdentity(current))
          });
        }
      },
      error: async (error, current) => {
        if (error instanceof SupersededPresetOperation) return;
        cancelInteraction({ kind: "tempo" });
        if (current.noticeTimer !== undefined) window.clearTimeout(current.noticeTimer);
        current.noticeTimer = undefined;
        current.lastAccepted = undefined;
        current.lastDetail = undefined;
        if (current.token) failCommand(current.token);
        fail(error);
        try {
          const snapshot = await gateway.currentSnapshot();
          lifetime.assertCurrent(origin);
          acceptSnapshot(snapshot);
        } catch { /* Preserve the command error. */ }
      },
      idle: (current) => {
        current.expected = undefined;
        current.original = undefined;
      },
      reschedule: () => void drainTempo()
    });
  }, [acceptSnapshot, cancelInteraction, fail, failCommand, gateway, lifetime, origin, recordHistory, snapshotRef]);

  const queueTempo = useCallback((requestedBpm: number, source: TempoSource = "Encoder") => {
    const bpm = Math.max(QC_MINIMUM_TEMPO_BPM, Math.min(QC_MAXIMUM_TEMPO_BPM, Math.round(requestedBpm)));
    if (demo) {
      settleCommand(beginTempo(bpm));
      tempo.current.source = source;
      tempo.current.lastAccepted = bpm;
      tempo.current.lastDetail = `Demo: ${source.toLocaleLowerCase()} tempo ${bpm} BPM.`;
      scheduleTempoNotice();
      return;
    }
    if (!connected) { notice("Connect the Quad Cortex before changing tempo."); return; }
    const queue = tempo.current;
    if (queue.expected === undefined) {
      queue.expected = snapshotRef.current.tempo;
      queue.original = queue.expected;
    }
    queue.target = bpm;
    queue.source = source;
    if (source === "Encoder") {
      updateInteraction({ kind: "tempo" }, bpm);
      scheduleTempoInteractionRelease(bpm);
    }
    queue.token = beginTempo(bpm);
    if (!queue.running) void drainTempo();
    scheduleTempoNotice();
  }, [beginTempo, connected, demo, drainTempo, notice, scheduleTempoInteractionRelease, scheduleTempoNotice, settleCommand, snapshotRef, updateInteraction]);

  const adjustTempo = useCallback((delta: number) => {
    queueTempo((tempo.current.target ?? snapshotRef.current.tempo) + delta, "Encoder");
  }, [queueTempo, snapshotRef]);

  const adoptVolumeReadback = useCallback((snapshot: PresetSnapshot) => {
    if (!canAdoptNativeState(snapshotRef.current.stateVersion, snapshot.stateVersion)) return;
    if (snapshot.stateVersion) acceptSnapshot(snapshot);
    // Some accepted Windows writes return an unversioned value before the
    // pushed native echo arrives. It is still a device observation and must
    // pass through the same gesture authority as the versioned event stream;
    // writing it directly into React state made an in-progress drag jump back
    // once for every delayed command response.
    else acceptInteractionObservation({kind: "master-volume"}, snapshot.masterVolume);
  }, [acceptInteractionObservation, acceptSnapshot, snapshotRef]);

  const failVolume = useCallback(async (error: unknown) => {
    const current = volume.current;
    cancelInteraction({ kind: "master-volume" });
    if (current.releaseTimer !== undefined) window.clearTimeout(current.releaseTimer);
    if (current.confirmationTimer !== undefined) window.clearTimeout(current.confirmationTimer);
    current.target = undefined;
    current.awaiting = undefined;
    current.desired = undefined;
    current.confirmed = undefined;
    current.gestureActive = false;
    current.explicit = false;
    current.releaseTimer = undefined;
    current.confirmationTimer = undefined;
    current.lastAccepted = undefined;
    current.lastDetail = undefined;
    fail(error);
    try {
      const snapshot = await gateway.currentSnapshot();
      if (lifetime.currentSession(origin)) adoptVolumeReadback(snapshot);
    } catch { /* Preserve the command error. */ }
  }, [adoptVolumeReadback, cancelInteraction, fail, gateway, lifetime, origin]);

  const drainVolume = useCallback(async () => {
    const queue = volume.current;
    if (queue.running || queue.awaiting !== undefined || queue.target === undefined || queue.confirmed === undefined) return;
    const target = queue.target;
    const expected = queue.confirmed;
    queue.target = undefined;
    queue.awaiting = target;
    queue.running = true;
    try {
      const result = await gateway.setMasterVolume(target, expected);
      if (!lifetime.currentSession(origin)) throw new SupersededPresetOperation();
      if (result.snapshot) adoptVolumeReadback(result.snapshot);
      // Realtime RPC completion means only that the native transport accepted
      // the bytes. The next guarded write must wait for the QC's own type-17
      // echo, otherwise it races the native snapshot and is rejected against
      // the value that was still active when the previous frame was sent.
      if (queue.awaiting === target) {
        queue.confirmationTimer = window.setTimeout(() => {
          queue.confirmationTimer = undefined;
          if (volume.current.awaiting === target) {
            void failVolume(new Error(`Master Volume ${target} was sent, but the Quad Cortex did not confirm it.`));
          }
        }, VOLUME_CONFIRMATION_TIMEOUT_MS);
      }
    } catch (error) {
      if (!(error instanceof SupersededPresetOperation)) await failVolume(error);
    } finally {
      queue.running = false;
      if (queue.awaiting === undefined && queue.target !== undefined) void drainVolume();
      else publishVolumeCompletion();
    }
  }, [adoptVolumeReadback, lifetime, failVolume, gateway, origin, publishVolumeCompletion]);

  useEffect(() => subscribeInteractionObservation({kind: "master-volume"}, (observed) => {
    const queue = volume.current;
    const value = Math.round(observed);
    if (queue.awaiting === undefined || value !== queue.awaiting) return;
    if (queue.confirmationTimer !== undefined) window.clearTimeout(queue.confirmationTimer);
    queue.confirmationTimer = undefined;
    queue.confirmed = value;
    queue.awaiting = undefined;
    queue.lastAccepted = value;
    queue.lastDetail = `Master Volume set to ${value}.`;
    if (queue.target !== undefined) void drainVolume();
    else publishVolumeCompletion();
  }), [drainVolume, publishVolumeCompletion, subscribeInteractionObservation]);

  const adjustMasterVolume = useCallback((delta: number) => {
    const queue = volume.current;
    const observed = snapshotRef.current.masterVolume;
    const value = Math.max(0, Math.min(100, Math.round((queue.desired ?? observed) + delta)));
    if (value === (queue.desired ?? observed)) return;
    queue.desired = value;
    if (demo) {
      queue.gestureActive = true;
      // Demo writes are already accepted locally. Going through snapshot
      // reconciliation here lets the active interaction lease protect the
      // previous value, so the notice advances while the visible knob snaps
      // back to its starting position. Commit the preview directly instead.
      updateSnapshot((current) => ({ ...current, masterVolume: value }));
      queue.lastAccepted = value;
      queue.lastDetail = `Demo: Master Volume ${value}.`;
      scheduleVolumeInteractionRelease();
      return;
    }
    if (!connected) { notice("Connect the Quad Cortex before changing Master Volume."); return; }
    if (queue.confirmed === undefined) queue.confirmed = observed;
    queue.gestureActive = true;
    updateInteraction({ kind: "master-volume" }, value);
    // The knob and its numeric readout follow the gesture synchronously. This
    // preview is deliberately independent of the completion notice below and
    // of the USB acknowledgement that will reconcile the authoritative state.
    updateSnapshot((current) => ({...current, masterVolume: value}));
    queue.target = value;
    if (!queue.running) void drainVolume();
    scheduleVolumeInteractionRelease();
  }, [connected, demo, drainVolume, notice, scheduleVolumeInteractionRelease, snapshotRef, updateInteraction, updateSnapshot]);

  const cancel = useCallback(() => {
    if (tempo.current.noticeTimer !== undefined) window.clearTimeout(tempo.current.noticeTimer);
    if (volume.current.releaseTimer !== undefined) window.clearTimeout(volume.current.releaseTimer);
    if (volume.current.confirmationTimer !== undefined) window.clearTimeout(volume.current.confirmationTimer);
    if (tempoReleaseTimer.current !== undefined) window.clearTimeout(tempoReleaseTimer.current);
    cancelInteraction({ kind: "master-volume" });
    cancelInteraction({ kind: "tempo" });
    tempo.current.target = undefined;
    volume.current.target = undefined;
    tempo.current = {running: false, source: "Encoder"};
    volume.current = {running: false, gestureActive: false, explicit: false};
  }, [cancelInteraction]);

  useEffect(() => cancel, [cancel]);
  useEffect(() => {
    const session = lifetime.checkpoint();
    return lifetime.subscribe(() => {
      if (!lifetime.currentSession(session)) { cancel(); return; }
      const old = tempo.current;
      old.target = undefined;
      tempo.current = {running: false, source: "Encoder"};
    });
  }, [cancel, lifetime, origin.session]);
  return { queueTempo, adjustTempo, adjustMasterVolume, beginMasterVolumeInteraction, endMasterVolumeInteraction, cancel };
}
