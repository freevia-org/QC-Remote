import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type Dispatch, type MutableRefObject, type SetStateAction } from "react";
import type { PresetSnapshot } from "@qc-remote/client";
import { adjacentPresetPosition, canAdoptNativeState, presetBoundaryDetail } from "@qc-remote/core";
import { PresetLifetime, SupersededPresetOperation, samePresetAddress, QcCommandCoordinator, type PresetAddress, type PresetOperation, type QcDeviceCommand, type QcCommandResult, type QcCommandToken, type QcDeviceTransport, type QcStateUpdate } from "@qc-remote/core";
import { interactionObservationOf, QcInteractionAuthority, type QcInteractionTarget } from "./interaction-authority.ts";

export interface QcController {
  snapshot: PresetSnapshot;
  snapshotRef: MutableRefObject<PresetSnapshot>;
  lifetime: PresetLifetime;
  setSnapshot: Dispatch<SetStateAction<PresetSnapshot>>;
  updateSnapshot(transform: (current: PresetSnapshot) => PresetSnapshot): void;
  beginScene(scene: number): QcCommandToken;
  beginPresetMove(delta: -1 | 1, name?: string): QcCommandToken;
  beginModeSlot(slot: 0 | 1 | 2): QcCommandToken;
  beginTempo(tempo: number): QcCommandToken;
  beginBypass(blockId: string, bypassed: boolean): QcCommandToken;
  beginFootswitch(index: number): QcCommandToken;
  failCommand(token: QcCommandToken): void;
  settleCommand(token: QcCommandToken): void;
  resetCommands(): void;
  beginInteraction(target: QcInteractionTarget, value: number): void;
  updateInteraction(target: QcInteractionTarget, value: number): void;
  releaseInteraction(target: QcInteractionTarget, value: number): void;
  cancelInteraction(target: QcInteractionTarget): void;
  acceptInteractionObservation(target: QcInteractionTarget, value: number, observedAt?: number): boolean;
  subscribeInteractionObservation(target: QcInteractionTarget, listener: (value: number) => void): () => void;
  reconcileFrame(states: readonly QcStateUpdate[], now?: number): { snapshot: PresetSnapshot; states: QcStateUpdate[] };
  acceptSnapshot(incoming: PresetSnapshot, observedAt?: number): PresetSnapshot;
  readSnapshot(read: () => Promise<PresetSnapshot>, options?: {sessionRecovery: true}): Promise<PresetSnapshot>;
  runScene(transport: QcDeviceTransport, scene: number): Promise<QcCommandResult>;
  runPresetMove(transport: QcDeviceTransport, delta: -1 | 1, name?: string): Promise<QcCommandResult>;
  runModeSlot(transport: QcDeviceTransport, slot: 0 | 1 | 2): Promise<QcCommandResult>;
  runTempo(transport: QcDeviceTransport, tempo: number): Promise<QcCommandResult>;
  runBypass(transport: QcDeviceTransport, blockId: string, row: number, column: number, bypassed: boolean): Promise<QcCommandResult>;
  runFootswitch(transport: QcDeviceTransport, index: number): Promise<QcCommandResult>;
  runDeviceCommand(transport: QcDeviceTransport, command: QcDeviceCommand): Promise<QcCommandResult>;
}

/** The listener-map key for one interaction target. */
function listenerKey(target: QcInteractionTarget): string {
  return target.kind === "parameter"
    ? `parameter:${target.row}:${target.column}:${target.parameterIndex}`
    : target.kind;
}

/** Shared realtime React controller used by every QC application shell. */
export function useQcController(initialSnapshot: PresetSnapshot): QcController {
  const [snapshot, commitReactSnapshot] = useState(initialSnapshot);
  const snapshotRef = useRef(initialSnapshot);
  const nativeSnapshotVersion = useRef(initialSnapshot.stateVersion);
  const coordinatorRef = useRef<QcCommandCoordinator | null>(null);
  const interactionAuthorityRef = useRef<QcInteractionAuthority | null>(null);
  const interactionObservationListeners = useRef(new Map<string, Set<(value: number) => void>>());
  const lifetimeRef = useRef<PresetLifetime | null>(null);
  if (!lifetimeRef.current) lifetimeRef.current = new PresetLifetime(initialSnapshot);
  const lifetime = lifetimeRef.current;
  useSyncExternalStore(useCallback((notify) => lifetime.subscribe(notify), [lifetime]), () => lifetime.version);
  const authoritativeAddress = useRef<PresetAddress>(initialSnapshot);
  type Move = {
    transport: QcDeviceTransport;
    delta: -1 | 1;
    expected: PresetSnapshot;
    target: PresetAddress;
    token: QcCommandToken;
    resolve: (result: QcCommandResult) => void;
    reject: (error: unknown) => void;
  };
  const moveChain = useRef<{ operation: PresetOperation; entries: Move[] } | undefined>(undefined);
  const reconciliationTimerRef = useRef<ReturnType<typeof globalThis.setTimeout> | undefined>(undefined);
  const reconciliationGenerationRef = useRef(0);
  if (!coordinatorRef.current) coordinatorRef.current = new QcCommandCoordinator();
  if (!interactionAuthorityRef.current) interactionAuthorityRef.current = new QcInteractionAuthority();

  // React may batch state changes while native frames continue arriving. Keep
  // the imperative reference authoritative and commit both in one operation.
  const commitPreview = useCallback<Dispatch<SetStateAction<PresetSnapshot>>>((action) => {
    const next = typeof action === "function" ? action(snapshotRef.current) : action;
    snapshotRef.current = next;
    commitReactSnapshot(next);
  }, []);

  const observeAddress = useCallback((next: PresetAddress) => {
    authoritativeAddress.current = { setlistKey: next.setlistKey, presetPosition: next.presetPosition };
    lifetime.observe(next);
  }, [lifetime]);

  const setSnapshot = useCallback<Dispatch<SetStateAction<PresetSnapshot>>>((action) => {
    const next = typeof action === "function" ? action(snapshotRef.current) : action;
    if (typeof action !== "function") nativeSnapshotVersion.current = next.stateVersion;
    observeAddress(next);
    commitPreview(next);
  }, [commitPreview, observeAddress]);

  const acceptSnapshot = useCallback((incoming: PresetSnapshot, observedAt?: number) => {
    if (!canAdoptNativeState(snapshotRef.current.stateVersion, incoming.stateVersion)) return snapshotRef.current;
    nativeSnapshotVersion.current = incoming.stateVersion;
    observeAddress(incoming);
    const protectedIncoming = interactionAuthorityRef.current!.protectSnapshot(snapshotRef.current, incoming, observedAt);
    const next = coordinatorRef.current!.reconcileSnapshot(protectedIncoming, observedAt);
    commitPreview(next);
    return next;
  }, [commitPreview, observeAddress]);

  const updateSnapshot = useCallback((transform: (current: PresetSnapshot) => PresetSnapshot) => {
    setSnapshot(transform);
  }, [setSnapshot]);

  const begin = useCallback((command: { snapshot: PresetSnapshot; token: QcCommandToken }) => {
    commitPreview(command.snapshot);
    return command.token;
  }, [commitPreview]);

  const beginScene = useCallback((scene: number) => begin(coordinatorRef.current!.beginScene(snapshotRef.current, scene)), [begin]);
  const beginPresetMove = useCallback((delta: -1 | 1, name?: string) => begin(coordinatorRef.current!.beginPresetMove(snapshotRef.current, delta, name)), [begin]);
  const beginModeSlot = useCallback((slot: 0 | 1 | 2) => begin(coordinatorRef.current!.beginModeSlot(snapshotRef.current, slot)), [begin]);
  const beginTempo = useCallback((tempo: number) => begin(coordinatorRef.current!.beginTempo(snapshotRef.current, tempo)), [begin]);
  const beginBypass = useCallback((blockId: string, bypassed: boolean) => begin(coordinatorRef.current!.beginBypass(snapshotRef.current, blockId, bypassed)), [begin]);
  const beginFootswitch = useCallback((index: number) => begin(coordinatorRef.current!.beginFootswitch(snapshotRef.current, index)), [begin]);

  const cancelAuthoritativeReconciliation = useCallback(() => {
    reconciliationGenerationRef.current += 1;
    if (reconciliationTimerRef.current !== undefined) globalThis.clearTimeout(reconciliationTimerRef.current);
    reconciliationTimerRef.current = undefined;
  }, []);

  const scheduleAuthoritativeReconciliation = useCallback((transport: QcDeviceTransport, forceOnce = false) => {
    cancelAuthoritativeReconciliation();
    const generation = reconciliationGenerationRef.current;
    let forceNext = forceOnce;
    const arm = (minimumDelay = 0): void => {
      const delay = coordinatorRef.current!.pendingReconciliationDelay();
      if ((delay === undefined && !forceNext) || generation !== reconciliationGenerationRef.current) return;
      const force = forceNext;
      forceNext = false;
      reconciliationTimerRef.current = globalThis.setTimeout(async () => {
        reconciliationTimerRef.current = undefined;
        if (generation !== reconciliationGenerationRef.current
          || (!force && coordinatorRef.current!.pendingReconciliationDelay() === undefined)) return;
        const observationStartedAt = Date.now();
        try {
          const incoming = await transport.currentSnapshot();
          if (generation !== reconciliationGenerationRef.current) return;
          acceptSnapshot(incoming, observationStartedAt);
          arm();
        } catch {
          // A transient refresh failure must not make optimistic state
          // permanent. Retry slowly while the same command remains pending.
          arm(1_000);
        }
      }, Math.max(minimumDelay, delay === undefined ? 0 : delay + 1));
    };
    arm();
  }, [acceptSnapshot, cancelAuthoritativeReconciliation]);

  const failCommand = useCallback((token: QcCommandToken) => {
    commitPreview((current) => coordinatorRef.current!.fail(current, token));
  }, [commitPreview]);
  const settleCommand = useCallback((token: QcCommandToken) => coordinatorRef.current!.settle(token), []);
  const resetCommands = useCallback(() => {
    cancelAuthoritativeReconciliation();
    coordinatorRef.current!.reset();
    interactionAuthorityRef.current!.reset();
    lifetime.reset();
  }, [cancelAuthoritativeReconciliation, lifetime]);

  const beginInteraction = useCallback((target: QcInteractionTarget, value: number) => {
    interactionAuthorityRef.current!.begin(target, value);
  }, []);
  const updateInteraction = useCallback((target: QcInteractionTarget, value: number) => {
    interactionAuthorityRef.current!.update(target, value);
  }, []);
  const releaseInteraction = useCallback((target: QcInteractionTarget, value: number) => {
    interactionAuthorityRef.current!.release(target, value);
  }, []);
  const cancelInteraction = useCallback((target: QcInteractionTarget) => {
    const observed = interactionAuthorityRef.current!.cancel(target);
    if (observed === undefined) return;
    if (target.kind === "master-volume") commitPreview((current) => ({ ...current, masterVolume: observed }));
    else if (target.kind === "tempo") commitPreview((current) => ({ ...current, tempo: observed }));
  }, [commitPreview]);

  const acceptInteractionObservation = useCallback((target: QcInteractionTarget, value: number, observedAt?: number) => {
    if (!interactionAuthorityRef.current!.accepts(target, value, observedAt)) return false;
    if (target.kind === "master-volume") commitPreview((current) => ({ ...current, masterVolume: value }));
    else if (target.kind === "tempo") commitPreview((current) => ({ ...current, tempo: value }));
    return true;
  }, [commitPreview]);

  const subscribeInteractionObservation = useCallback((target: QcInteractionTarget, listener: (value: number) => void) => {
    const key = listenerKey(target);
    const listeners = interactionObservationListeners.current.get(key) ?? new Set<(value: number) => void>();
    listeners.add(listener);
    interactionObservationListeners.current.set(key, listeners);
    return () => {
      listeners.delete(listener);
      if (!listeners.size) interactionObservationListeners.current.delete(key);
    };
  }, []);

  useEffect(() => () => resetCommands(), [resetCommands]);

  useEffect(() => lifetime.subscribe(() => {
    cancelAuthoritativeReconciliation();
    const chain = moveChain.current;
    if (chain && !lifetime.current(chain.operation)) {
      moveChain.current = undefined;
      lifetime.end(chain.operation);
      for (const entry of chain.entries.splice(0)) entry.reject(new SupersededPresetOperation());
    }
  }), [cancelAuthoritativeReconciliation, lifetime]);

  const reconcileFrame = useCallback((states: readonly QcStateUpdate[], now?: number) => {
    let latest = snapshotRef.current.stateVersion;
    const ordered = states.filter((state) => {
      if (latest && latest.session !== state.stateVersion?.session) return false;
      // A complete snapshot covers every older delta. Another partial event
      // does not: it may concern an entirely different field. Cross-field
      // native publication ordering remains the hosts' responsibility.
      if (!canAdoptNativeState(nativeSnapshotVersion.current, state.stateVersion)) return false;
      if (canAdoptNativeState(latest, state.stateVersion)) latest = state.stateVersion ?? latest;
      return true;
    });
    for (const state of ordered) {
      const observation = interactionObservationOf(state);
      if (!observation) continue;
      for (const listener of interactionObservationListeners.current.get(listenerKey(observation.target)) ?? []) listener(observation.value);
    }
    const accepted = ordered.filter((state) => interactionAuthorityRef.current!.acceptsState(state, now));
    const reduced = coordinatorRef.current!.reconcileFrame(snapshotRef.current, accepted, now);
    // Suppressing an old value in favor of a pending local intent does not make
    // its native observation disappear. Slower snapshots must still respect it.
    if (latest !== reduced.snapshot.stateVersion) reduced.snapshot = {...reduced.snapshot, stateVersion: latest};
    for (const state of ordered) {
      if (state.kind !== "position" || state.position === undefined) continue;
      const address = {setlistKey: state.setlistKey ?? authoritativeAddress.current.setlistKey, presetPosition: state.position};
      const expectedTarget = moveChain.current?.operation.target;
      // Rapid navigation may suppress the first step's display while a later
      // step is previewed. Its expected acknowledgement still advances the
      // chain; other suppressed stale echoes must not advance the lifetime.
      if (reduced.states.includes(state) || (expectedTarget && samePresetAddress(expectedTarget, address))) observeAddress(address);
    }
    if (reduced.states.length || reduced.snapshot !== snapshotRef.current) commitPreview(reduced.snapshot);
    return reduced;
  }, [commitPreview, observeAddress]);

  const readSnapshot = useCallback(async (read: () => Promise<PresetSnapshot>, options?: {sessionRecovery: true}) => {
    const operation = lifetime.begin();
    const observedAt = Date.now();
    const sourceSession = snapshotRef.current.stateVersion?.session;
    try {
      const incoming = await read();
      lifetime.assertCurrent(operation);
      if (options?.sessionRecovery && incoming.stateVersion
        && incoming.stateVersion.session !== snapshotRef.current.stateVersion?.session
        && canAdoptNativeState(undefined, incoming.stateVersion)) {
        // Only an explicit fresh recovery read may install another native
        // session. A late read cannot replace a session installed meanwhile.
        if (snapshotRef.current.stateVersion?.session !== sourceSession) return snapshotRef.current;
        resetCommands();
        setSnapshot(incoming);
        return incoming;
      }
      return acceptSnapshot(incoming, observedAt);
    } catch (error) {
      lifetime.assertCurrent(operation);
      throw error;
    } finally {
      lifetime.end(operation);
    }
  }, [acceptSnapshot, lifetime, resetCommands, setSnapshot]);

  const runCommand = useCallback(async (
    transport: QcDeviceTransport,
    preview: () => QcCommandToken,
    send: (expected: PresetSnapshot) => Promise<QcCommandResult>
  ): Promise<QcCommandResult> => {
    const expected = snapshotRef.current;
    const token = preview();
    const operation = lifetime.begin(samePresetAddress(expected, snapshotRef.current) ? undefined : snapshotRef.current);
    try {
      const result = await send(expected);
      lifetime.assertCurrent(operation);
      if (result.snapshot) acceptSnapshot(result.snapshot);
      // A newer intent that superseded this one now rolls back to this result.
      const lateSuccess = coordinatorRef.current!.acknowledge(token);
      if (coordinatorRef.current!.hasPendingCommands || lateSuccess) scheduleAuthoritativeReconciliation(transport, lateSuccess);
      return result;
    } catch (error) {
      lifetime.assertCurrent(operation);
      // The coordinator restores the device's authoritative value, never an
      // older optimistic one; any newer intent still pending is reconciled.
      commitPreview((current) => coordinatorRef.current!.fail(current, token));
      if (coordinatorRef.current!.hasPendingCommands) scheduleAuthoritativeReconciliation(transport);
      throw error;
    } finally {
      lifetime.end(operation);
    }
  }, [acceptSnapshot, commitPreview, lifetime, scheduleAuthoritativeReconciliation]);

  const runScene = useCallback((transport: QcDeviceTransport, scene: number) => runCommand(transport,
    () => beginScene(scene),
    (expected) => transport.selectScene(scene, expected)
  ), [beginScene, runCommand]);
  const runPresetMove = useCallback((transport: QcDeviceTransport, delta: -1 | 1, name?: string): Promise<QcCommandResult> => {
    const expected = snapshotRef.current;
    // A step past either end of the setlist neither previews nor recalls.
    if (adjacentPresetPosition(expected.presetPosition, delta) === expected.presetPosition) {
      return Promise.resolve({ detail: presetBoundaryDetail(delta) });
    }
    const token = beginPresetMove(delta, name);
    const target = { setlistKey: snapshotRef.current.setlistKey, presetPosition: snapshotRef.current.presetPosition };
    const existing = moveChain.current;
    const chain = existing ?? { operation: lifetime.begin(target), entries: [] as Move[] };
    const promise = new Promise<QcCommandResult>((resolve, reject) => {
      chain.entries.push({ transport, delta, expected, target, token, resolve, reject });
    });
    if (!existing) {
      moveChain.current = chain;
      void (async () => {
        try {
          while (chain.entries.length) {
            const entry = chain.entries[0];
            lifetime.follow(chain.operation, entry.target);
            const result = await entry.transport.movePreset(entry.delta, entry.expected);
            lifetime.assertCurrent(chain.operation);
            if (result.snapshot) acceptSnapshot(result.snapshot);
            const lateSuccess = coordinatorRef.current!.acknowledge(entry.token);
            if (coordinatorRef.current!.hasPendingCommands || lateSuccess) scheduleAuthoritativeReconciliation(entry.transport, lateSuccess);
            chain.entries.shift();
            entry.resolve(result);
            const nextEntry = chain.entries[0];
            if (nextEntry) {
              // Only the preceding step's readback may authorize the next
              // queued address. A refresh of an unrelated preset cannot.
              if (!result.snapshot || !samePresetAddress(result.snapshot, nextEntry.expected)) {
                throw new SupersededPresetOperation();
              }
              nextEntry.expected = { ...nextEntry.expected, presetIdentity: result.snapshot.presetIdentity };
            }
          }
        } catch (error) {
          const failed = chain.entries.splice(0);
          if (lifetime.current(chain.operation)) {
            commitPreview((current) => failed.reduce(
              (state, pending) => coordinatorRef.current!.fail(state, pending.token),
              current
            ));
          }
          for (const pending of failed) pending.reject(error);
        } finally {
          lifetime.end(chain.operation);
          if (moveChain.current === chain) moveChain.current = undefined;
        }
      })();
    }
    return promise;
  }, [acceptSnapshot, beginPresetMove, commitPreview, lifetime, scheduleAuthoritativeReconciliation]);
  const runModeSlot = useCallback((transport: QcDeviceTransport, slot: 0 | 1 | 2) => runCommand(transport,
    () => beginModeSlot(slot),
    (expected) => transport.selectModeSlot(slot, expected)
  ), [beginModeSlot, runCommand]);
  const runTempo = useCallback((transport: QcDeviceTransport, tempo: number) => runCommand(transport,
    () => beginTempo(tempo),
    (expected) => transport.setTempo(tempo, expected)
  ), [beginTempo, runCommand]);
  const runBypass = useCallback((transport: QcDeviceTransport, blockId: string, row: number, column: number, bypassed: boolean) => runCommand(transport,
    () => beginBypass(blockId, bypassed),
    (expected) => transport.setBypass(row, column, bypassed, expected)
  ), [beginBypass, runCommand]);
  const runFootswitch = useCallback((transport: QcDeviceTransport, index: number) => runCommand(transport,
    () => beginFootswitch(index),
    (expected) => transport.pressFootswitch(index, expected)
  ), [beginFootswitch, runCommand]);
  const runDeviceCommand = useCallback((transport: QcDeviceTransport, command: QcDeviceCommand) => {
    if (command.kind === "scene") return runScene(transport, command.scene);
    if (command.kind === "preset-step") return runPresetMove(transport, command.delta);
    if (command.kind === "tempo") return runTempo(transport, command.bpm);
    if (command.kind === "bypass") return runBypass(transport, command.blockId, command.row, command.column, command.bypassed);
    return command.view === "tuner"
      ? transport.setTuner(command.show, snapshotRef.current)
      : transport.setGigView(command.show, snapshotRef.current);
  }, [runBypass, runPresetMove, runScene, runTempo]);

  // Every member but `snapshot` is a stable callback or ref, so the
  // controller is a new object only when the snapshot itself moves. A literal
  // here gave consumers a new controller on every render of the host.
  return useMemo(() => ({
    snapshot,
    snapshotRef,
    lifetime,
    setSnapshot,
    updateSnapshot,
    beginScene,
    beginPresetMove,
    beginModeSlot,
    beginTempo,
    beginBypass,
    beginFootswitch,
    failCommand,
    settleCommand,
    resetCommands,
    beginInteraction,
    updateInteraction,
    releaseInteraction,
    cancelInteraction,
    acceptInteractionObservation,
    subscribeInteractionObservation,
    reconcileFrame,
    acceptSnapshot,
    readSnapshot,
    runScene,
    runPresetMove,
    runModeSlot,
    runTempo,
    runBypass,
    runFootswitch,
    runDeviceCommand
  }), [
    snapshot, snapshotRef, lifetime, setSnapshot, updateSnapshot, beginScene, beginPresetMove, beginModeSlot,
    beginTempo, beginBypass, beginFootswitch, failCommand, settleCommand, resetCommands, beginInteraction,
    updateInteraction, releaseInteraction, cancelInteraction, acceptInteractionObservation,
    subscribeInteractionObservation, reconcileFrame, acceptSnapshot, readSnapshot, runScene, runPresetMove,
    runModeSlot, runTempo, runBypass, runFootswitch, runDeviceCommand
  ]);
}
