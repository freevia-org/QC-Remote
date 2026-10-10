import { useEffect, useRef, type MutableRefObject } from "react";
import type { NativeStateFrames, PresetSnapshot } from "@qc-remote/client";
import { SupersededPresetOperation } from "@qc-remote/core";
import { isIncompleteGridRefreshFailure, observeCompleteGrid, runGridRepairOnce, type GridRepairCoordinator, type GridRepairEpisode } from "./grid-snapshot-refresh.ts";
import type { QcController } from "./use-qc-controller";

export interface NativeStateRecoveryOptions {
  enabled: boolean;
  controller: QcController;
  sequence: MutableRefObject<number>;
  streamId: MutableRefObject<string | undefined>;
  recoveryRevision?: MutableRefObject<number>;
  gridRepair: MutableRefObject<GridRepairCoordinator>;
  readEvents(afterSequence: number, limit: number): Promise<NativeStateFrames>;
  readSnapshot(): Promise<PresetSnapshot>;
  readGridRepair(): Promise<PresetSnapshot>;
  onRecoveryResult?(result: NativeStateRecoveryResult): void;
  intervalMs?: number;
}

export type NativeStateRecoveryResult =
  | {status: "recovered"; revision: number; streamId: string | undefined; snapshot: PresetSnapshot}
  | {status: "failed"; revision: number; streamId: string | undefined; error: unknown};

const JOURNAL_PAGE_SIZE = 64;
const MAX_JOURNAL_PAGES_PER_TICK = 8;

function latestGridInvalidationSequence(frames: NativeStateFrames["frames"]): number | undefined {
  let latestTopology: number | undefined;
  let latestFullPreset: number | undefined;
  for (const frame of frames) {
    for (const state of frame.states) {
      if (!state || typeof state !== "object") continue;
      const update = state as {kind?: unknown; gridLayoutComplete?: unknown};
      if (update.kind === "gridTopology" && update.gridLayoutComplete === false) latestTopology = frame.sequence;
      if (update.kind === "preset") latestFullPreset = frame.sequence;
    }
  }
  return latestTopology !== undefined && (latestFullPreset === undefined || latestTopology > latestFullPreset)
    ? latestTopology : undefined;
}

/** Catch a dropped final notification and bootstrap an already-connected host.
 * Event pages are cache reads. A failed incomplete-Grid repair is latched to
 * its producer and topology event until a newer topology event or producer.
 */
export function useNativeStateRecovery(options: NativeStateRecoveryOptions): void {
  const latest = useRef(options);
  const ownRecoveryRevision = useRef(0);
  latest.current = options;
  const {enabled, sequence, streamId, intervalMs = 2_000} = options;
  const recoveryRevision = options.recoveryRevision ?? ownRecoveryRevision;
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let initialized = false;
    let recoveredStream: string | undefined;
    let recoveredRevision = -1;
    let journalScanStream: string | undefined;
    let journalScanSequence = sequence.current;
    let timer: ReturnType<typeof globalThis.setTimeout> | undefined;
    const poll = async () => {
      const current = latest.current;
      // Event-triggered and recovery-triggered reads share one admission. Do
      // not reset the controller lifetime while either owner is still reading.
      if (current.gridRepair.current.inFlight) {
        if (!disposed) timer = globalThis.setTimeout(() => void poll(), intervalMs);
        return;
      }
      let lifetime = current.controller.lifetime.checkpoint();
      const requestedSequence = sequence.current;
      const requestedStream = streamId.current;
      const requestedRecovery = recoveryRevision.current;
      let observedGridInvalidationSequence: number | undefined;
      let observedProducer: string | undefined;
      try {
        let page = await current.readEvents(journalScanSequence, JOURNAL_PAGE_SIZE);
        if (disposed || !current.controller.lifetime.current(lifetime)) return;
        let tail = page.latestSequence;
        if (!page.native || typeof tail !== "number" || !Number.isSafeInteger(tail) || tail < 0) return;
        let producer = typeof page.streamId === "string" && page.streamId.length ? page.streamId : undefined;
        if (journalScanStream !== producer) {
          const hadProducer = journalScanStream !== undefined;
          journalScanStream = producer;
          journalScanSequence = hadProducer ? 0 : requestedSequence;
          page = await current.readEvents(journalScanSequence, JOURNAL_PAGE_SIZE);
          if (disposed || !current.controller.lifetime.current(lifetime)) return;
          if (!page.native || !Number.isSafeInteger(page.latestSequence) || (page.latestSequence ?? -1) < 0) return;
          const rereadProducer = typeof page.streamId === "string" && page.streamId.length ? page.streamId : undefined;
          if (producer !== undefined && rereadProducer !== producer) return;
          tail = Math.max(tail, page.latestSequence!);
          producer = rereadProducer ?? producer;
        }
        // A live frame may have established another producer while this page
        // was in flight. Its old journal cannot move the new stream's cursor.
        if (streamId.current !== requestedStream && producer !== streamId.current) return;
        observedProducer = producer;
        const scannedPages = [page];
        for (let pageIndex = 1; pageIndex < MAX_JOURNAL_PAGES_PER_TICK && page.frames.length === JOURNAL_PAGE_SIZE; pageIndex += 1) {
          const lastScanned = Math.max(...page.frames.map((frame) => frame.sequence));
          page = await current.readEvents(lastScanned, JOURNAL_PAGE_SIZE);
          if (disposed || !current.controller.lifetime.current(lifetime)) return;
          const pageProducer = typeof page.streamId === "string" && page.streamId.length ? page.streamId : undefined;
          if (!page.native || pageProducer !== producer || !Number.isSafeInteger(page.latestSequence) || (page.latestSequence ?? -1) < 0) return;
          if (streamId.current !== requestedStream && streamId.current !== producer) return;
          tail = Math.max(tail, page.latestSequence!);
          scannedPages.push(page);
        }
        const scannedFrames = scannedPages.flatMap((item) => item.frames);
        if (scannedFrames.length) journalScanSequence = Math.max(journalScanSequence, ...scannedFrames.map((frame) => frame.sequence));
        observedGridInvalidationSequence = latestGridInvalidationSequence(scannedFrames);
        if (observedGridInvalidationSequence !== undefined && observedGridInvalidationSequence <= requestedSequence
            && current.controller.snapshotRef.current.gridLayoutComplete !== false) {
          observedGridInvalidationSequence = undefined;
        }
        if (observedGridInvalidationSequence !== undefined) {
          current.controller.updateSnapshot((snapshot) => snapshot.gridLayoutComplete === false
            ? snapshot
            : {...snapshot, gridLayoutComplete: false});
        }
        const replaced = producer !== undefined && (producer !== streamId.current || producer !== recoveredStream);
        const missed = tail > sequence.current || (producer === undefined && tail < requestedSequence);
        const gridComplete = current.controller.snapshotRef.current.gridLayoutComplete;
        if (initialized && !replaced && !missed && recoveredRevision === requestedRecovery
            && observedGridInvalidationSequence === undefined && gridComplete !== false) return;
        let snapshot: PresetSnapshot | undefined;
        let repairAttemptEpisode: GridRepairEpisode | undefined;
        let commandsReset = false;
        const repairGrid = (invalidationSequence: number | undefined) => {
          const repair = runGridRepairOnce(
            current.gridRepair.current,
            producer ?? streamId.current ?? requestedStream,
            invalidationSequence,
            async () => {
              if ((missed || initialized) && !commandsReset) {
                current.controller.resetCommands();
                lifetime = current.controller.lifetime.checkpoint();
                commandsReset = true;
              }
              return current.controller.readSnapshot(async () => {
                const incoming = await current.readGridRepair();
                if (disposed) throw new SupersededPresetOperation();
                return incoming;
              }, {sessionRecovery: true});
            }
          );
          repairAttemptEpisode = {
            producer: current.gridRepair.current.producer,
            invalidationSequence: current.gridRepair.current.invalidationSequence,
            episode: current.gridRepair.current.episode
          };
          return repair;
        };
        if (observedGridInvalidationSequence !== undefined) {
          snapshot = await repairGrid(observedGridInvalidationSequence);
          if (!snapshot) return;
        } else {
          if (current.gridRepair.current.inFlight) return;
          if (missed || initialized) {
          // The missing delta could belong to an open editor or its history,
          // which a preset snapshot alone cannot refresh. Revoke that work as
          // the immediate stream-gap handler does. This path is cache-only.
            current.controller.resetCommands();
            lifetime = current.controller.lifetime.checkpoint();
            commandsReset = true;
          }
          snapshot = await current.controller.readSnapshot(async () => {
            const incoming = await current.readSnapshot();
            if (disposed) throw new SupersededPresetOperation();
            return incoming;
          }, {sessionRecovery: true});
          if (current.gridRepair.current.inFlight) return;
          if (snapshot.gridLayoutComplete === false) {
            current.controller.updateSnapshot((value) => value.gridLayoutComplete === false
              ? value
              : {...value, gridLayoutComplete: false});
            const repaired = await repairGrid(undefined);
            if (!repaired) return;
            snapshot = repaired;
          }
        }
        if (snapshot.gridLayoutComplete === false) throw new Error("The Grid layout is incomplete after native current-preset repair.");
        if (current.gridRepair.current.inFlight) return;
        if (disposed) return;
        if (recoveryRevision.current !== requestedRecovery) return;
        // A new frame can arrive while the checked snapshot is in flight. Do
        // not publish recovery completion for a snapshot that belongs to its
        // predecessor's stream.
        if (streamId.current !== requestedStream && streamId.current !== producer) return;
        observeCompleteGrid(current.gridRepair.current, repairAttemptEpisode);
        // A successful complete read covers at least the tail observed before
        // it began. Keep later live frames; do not replay a journal of deltas.
        if (producer !== undefined) {
          sequence.current = streamId.current === producer ? Math.max(sequence.current, tail) : tail;
          streamId.current = producer;
        } else {
          sequence.current = tail < requestedSequence ? tail : Math.max(sequence.current, tail);
        }
        initialized = true;
        recoveredStream = producer;
        recoveredRevision = requestedRecovery;
        current.onRecoveryResult?.({status: "recovered", revision: requestedRecovery, streamId: streamId.current, snapshot});
      } catch (error) {
        if (disposed || !current.controller.lifetime.current(lifetime)
            || (streamId.current !== requestedStream && streamId.current !== observedProducer)) return;
        // A failed admitted Grid repair consumes its episode even when the
        // device call failed ambiguously. Ordinary cached recovery remains retryable.
        if (isIncompleteGridRefreshFailure(error)) {
          current.controller.updateSnapshot((snapshot) => snapshot.gridLayoutComplete === false
            ? snapshot
            : {...snapshot, gridLayoutComplete: false});
          // The explicit read was admitted by runGridRepairOnce before it
          // failed; do not re-admit the same unknown episode here.
        }
        if (!disposed && recoveryRevision.current === requestedRecovery
          && streamId.current === requestedStream && !(error instanceof SupersededPresetOperation)) {
          latest.current.onRecoveryResult?.({status: "failed", revision: requestedRecovery, streamId: requestedStream, error});
        }
      } finally {
        if (!disposed) timer = globalThis.setTimeout(() => void poll(), intervalMs);
      }
    };
    void poll();
    return () => {
      disposed = true;
      if (timer !== undefined) globalThis.clearTimeout(timer);
    };
  }, [enabled, intervalMs, sequence, streamId, recoveryRevision]);
}
