import { requireModelId, requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useEffect, useRef } from "react";
import type { BlockDetails, BlockParameter, GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { BlockEditorSessionController } from "./use-block-editor-session";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";
import type { QcController } from "./use-qc-controller";
import { samePresetAddress, SupersededPresetOperation } from "@qc-remote/core";
import { PARAMETER_ENCODER_ROLES, parameterEditorControlSlots, parameterEditorPageSize, parameterStep } from "./parameter-model.ts";

type ParameterGesture = {
  details: BlockDetails;
  parameter: BlockParameter;
  snapshot: PresetSnapshot;
  gateway: GatewayTransport;
  observed: number;
  sent: number[];
};
type Preview = { gesture: ParameterGesture; value: number; revision: number };
type Release = { revision: number; apply(): Promise<void> };
const sameParameterValue = (left: number, right: number) => Math.abs(left - right) <= .00001;
// Stop speculative previews if the native cache has not observed this bounded
// window. Release still reads current state and performs a verified write.
const maximumUnobservedPreviews = 32;

export interface ParameterWorkflowOptions extends DeviceMutationWorkflowOptions {
  editor: BlockEditorSessionController;
  controller: QcController;
}

/** Shared low-latency parameter preview, coalescing, verified commit, and recovery. */
export function useParameterWorkflow(options: ParameterWorkflowOptions) {
  const { gateway, snapshot, editor, controller, connected, pending, setPending, reconcile, recordHistory, notice, fail } = options;
  const timers = useRef(new Map<number, number>());
  const targets = useRef(new Map<number, number>());
  const clock = useRef(0);
  const revisions = useRef(new Map<number, number>());
  const previewQueue = useRef<Preview | undefined>(undefined);
  const previewRunning = useRef(false);
  const previewWaiters = useRef<Array<() => void>>([]);
  const gestures = useRef(new Map<number, ParameterGesture>());
  const authorityTargets = useRef(new Map<number, { kind: "parameter"; row: number; column: number; parameterIndex: number }>());
  const releases = useRef(new Map<number, Release>());
  const releaseRunning = useRef(false);
  const batchRunning = useRef(false);
  const lifetime = useRef(0);
  const context = useRef({snapshot, connected});
  context.current = {snapshot, connected};
  const detailsRef = useRef(editor.details);
  useEffect(() => { detailsRef.current = editor.details; }, [editor.details]);

  const resolveWaiters = useCallback(() => {
    if (previewRunning.current || previewQueue.current) return;
    for (const resolve of previewWaiters.current.splice(0)) resolve();
  }, []);

  const gestureCurrent = useCallback((gesture: ParameterGesture) => {
    const current = context.current;
    const details = detailsRef.current;
    return gestures.current.get(gesture.parameter.index) === gesture && current.connected
      && samePresetAddress(current.snapshot, gesture.snapshot)
      && current.snapshot.presetIdentity === gesture.snapshot.presetIdentity
      && current.snapshot.activeScene === gesture.snapshot.activeScene
      && details?.row === gesture.details.row && details.column === gesture.details.column
      && details.modelId === gesture.details.modelId && details.scene === gesture.details.scene;
  }, []);

  const observeGesture = useCallback((gesture: ParameterGesture, details: BlockDetails) => {
    if (!gestureCurrent(gesture)) throw new SupersededPresetOperation();
    if (details.row !== gesture.details.row || details.column !== gesture.details.column
        || details.modelId !== gesture.details.modelId || details.scene !== gesture.details.scene) {
      throw new Error("The parameter's block or scene changed. Select it again before editing.");
    }
    const parameter = details.parameters.find((item) => item.index === gesture.parameter.index);
    const actual = parameter?.normalizedValue;
    if (!parameter || actual == null || !Number.isFinite(actual) || !parameter.writable) {
      throw new Error("The parameter no longer has a writable synchronized value.");
    }
    const observedSend = gesture.sent.findIndex((value) => sameParameterValue(value, actual));
    if (!sameParameterValue(gesture.observed, actual) && observedSend < 0) {
      detailsRef.current = details;
      editor.load(details);
      throw new Error("The parameter changed outside this gesture. Refresh and retry.");
    }
    if (observedSend >= 0) gesture.sent.splice(0, observedSend + 1);
    gesture.observed = actual;
    return {details, parameter};
  }, [editor.load, gestureCurrent]);

  const readGesture = useCallback(async (gesture: ParameterGesture) => {
    if (!gestureCurrent(gesture)) throw new SupersededPresetOperation();
    const details = await gesture.gateway.blockDetails(gesture.details.row, gesture.details.column,
      gesture.snapshot.presetName, requirePresetIdentity(gesture.snapshot));
    return observeGesture(gesture, details);
  }, [gestureCurrent, observeGesture]);

  const drainPreviews = useCallback(async () => {
    if (previewRunning.current) return;
    previewRunning.current = true;
    try {
      while (previewQueue.current) {
        const next = previewQueue.current;
        previewQueue.current = undefined;
        try {
          const {gesture} = next;
          const {parameter} = await readGesture(gesture);
          if (!gestureCurrent(gesture) || revisions.current.get(parameter.index) !== next.revision) continue;
          if (sameParameterValue(parameter.normalizedValue!, next.value)
              || gesture.sent.length >= maximumUnobservedPreviews) continue;
          // Record the attempted value, including ambiguous delivery. Only a
          // subsequent native observation can turn it into an admission value.
          gesture.sent.push(next.value);
          await gesture.gateway.previewParameter(gesture.details.row, gesture.details.column,
            parameter.index, next.value, parameter.normalizedValue!, gesture.snapshot.activeScene,
            requireModelId(gesture.details), gesture.snapshot.presetName, requirePresetIdentity(gesture.snapshot));
        } catch { /* The verified release write owns reconciliation and errors. */ }
      }
    } finally {
      previewRunning.current = false;
      resolveWaiters();
    }
  }, [gestureCurrent, readGesture, resolveWaiters]);

  const waitForPreviews = useCallback(() => !previewRunning.current && !previewQueue.current
    ? Promise.resolve()
    : new Promise<void>((resolve) => previewWaiters.current.push(resolve)), []);

  const cancel = useCallback(() => {
    lifetime.current++;
    clock.current += 1;
    revisions.current.clear();
    targets.current.clear();
    previewQueue.current = undefined;
    for (const target of authorityTargets.current.values()) controller.cancelInteraction(target);
    authorityTargets.current.clear();
    gestures.current.clear();
    releases.current.clear();
    for (const timer of timers.current.values()) window.clearTimeout(timer);
    timers.current.clear();
    resolveWaiters();
  }, [controller.cancelInteraction, resolveWaiters]);

  useEffect(() => cancel, [cancel, connected, snapshot.presetIdentity, snapshot.setlistKey,
    snapshot.presetPosition, snapshot.activeScene, editor.details?.row, editor.details?.column,
    editor.details?.modelId, editor.details?.scene]);

  const applyResolvedParameter = useCallback(async (
    details: BlockDetails,
    parameter: BlockParameter,
    value: number,
    reportFailure = false,
    stillCurrent: () => boolean = () => true,
    gesture?: ParameterGesture
  ): Promise<string | undefined> => {
    if (snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block parameter edits are paused until the current-preset refresh succeeds.");
      return;
    }
    if (parameter.normalizedValue === null || (pending && !reportFailure && !gesture)) return;
    if (!gesture && Math.abs(value - parameter.normalizedValue) < .000001) return;
    const originalValue = gesture?.parameter.normalizedValue ?? parameter.normalizedValue;
    const operationSnapshot = gesture?.snapshot ?? snapshot;
    const operationGateway = gesture?.gateway ?? gateway;
    if (!connected) {
      editor.updateParameter(parameter, value);
      reconcile({ ...snapshot, dirty: true });
      const detail = `Preview: ${details.name} · ${parameter.name} adjusted.`;
      notice(detail);
      return detail;
    }
    const row = details.row;
    const column = details.column;
    setPending(true);
    notice(`Applying ${parameter.name}…`);
    try {
      const result = await operationGateway.setParameter(row, column, parameter.index, value, parameter.normalizedValue,
        operationSnapshot.activeScene, requireModelId(details), operationSnapshot.presetName, requirePresetIdentity(operationSnapshot));
      if (stillCurrent()) {
        if (gesture && (result.block.row !== details.row || result.block.column !== details.column
            || result.block.modelId !== details.modelId || result.block.scene !== details.scene)) {
          throw new Error("The parameter's block or scene changed before release confirmation.");
        }
        editor.load(result.block);
        detailsRef.current = result.block;
        if (result.snapshot) reconcile(result.snapshot);
        const confirmed = result.block.parameters.find((item) => item.index === parameter.index)?.normalizedValue;
        if (gesture && (confirmed == null || !sameParameterValue(confirmed, value))) {
          throw new Error("The parameter changed before release confirmation. No undo entry was recorded.");
        }
        if (!sameParameterValue(originalValue, value)) recordHistory({
          label: `${details.name} ${parameter.name}`,
          execute: (current) => operationGateway.setParameter(row, column, parameter.index, originalValue, value,
            operationSnapshot.activeScene, requireModelId(details), current.presetName, requirePresetIdentity(operationSnapshot)),
          redo: (current) => operationGateway.setParameter(row, column, parameter.index, value, originalValue,
            operationSnapshot.activeScene, requireModelId(details), current.presetName, requirePresetIdentity(operationSnapshot))
        });
        notice(result.detail);
        return result.detail;
      }
    } catch (error) {
      if (gesture && stillCurrent()) {
        try {
          const observed = await operationGateway.blockDetails(row, column,
            operationSnapshot.presetName, requirePresetIdentity(operationSnapshot));
          if (stillCurrent() && observed.row === details.row && observed.column === details.column
              && observed.modelId === details.modelId && observed.scene === details.scene) {
            detailsRef.current = observed;
            editor.load(observed);
          }
        } catch { /* Keep the original error; recovery never retries a write. */ }
      }
      if (reportFailure) throw error;
      if (stillCurrent()) fail(error);
    } finally {
      setPending(false);
    }
  }, [connected, editor, fail, gateway, notice, pending, reconcile, recordHistory, setPending, snapshot]);

  const apply = useCallback(async (parameter: BlockParameter, value: number, revision: number) => {
    if (snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block parameter edits are paused until the current-preset refresh succeeds.");
      return;
    }
    const gesture = gestures.current.get(parameter.index);
    const details = detailsRef.current;
    if (!details) return;
    const stillCurrent = () => revisions.current.get(parameter.index) === revision
      && detailsRef.current?.row === details.row && detailsRef.current?.column === details.column
      && detailsRef.current?.modelId === details.modelId && detailsRef.current?.scene === details.scene
      && (!gesture || gestureCurrent(gesture));
    try {
      const observed = gesture ? await readGesture(gesture) : {details, parameter};
      if (!stillCurrent()) return;
      if (gesture) gesture.sent.push(value);
      await applyResolvedParameter(observed.details, observed.parameter, value, true, stillCurrent, gesture);
    } catch (error) {
      if (stillCurrent()) fail(error);
    } finally {
      if (stillCurrent()) {
        targets.current.delete(parameter.index);
        gestures.current.delete(parameter.index);
      }
    }
  }, [applyResolvedParameter, fail, gestureCurrent, readGesture]);

  const drainReleases = useCallback(async () => {
    if (releaseRunning.current || batchRunning.current) return;
    releaseRunning.current = true;
    try {
      while (releases.current.size) {
        const [index, next] = releases.current.entries().next().value!;
        releases.current.delete(index);
        await waitForPreviews();
        if (revisions.current.get(index) === next.revision) await next.apply();
      }
    } finally {
      releaseRunning.current = false;
    }
  }, [waitForPreviews]);

  const draft = useCallback((parameter: BlockParameter, value: number) => {
    if (batchRunning.current) return clock.current;
    const revision = ++clock.current;
    revisions.current.set(parameter.index, revision);
    targets.current.set(parameter.index, value);
    editor.draft(parameter, value);
    const authorityDetails = detailsRef.current;
    if (authorityDetails) {
      const target = { kind: "parameter" as const, row: authorityDetails.row, column: authorityDetails.column, parameterIndex: parameter.index };
      authorityTargets.current.set(parameter.index, target);
      controller.updateInteraction(target, value);
    }
    if (connected && !snapshot.dirty) reconcile({ ...snapshot, dirty: true });
    const continuous = !parameter.options.length && ["float", "floatwithled", "int", "fader"].includes(parameter.type.toLocaleLowerCase());
    const details = detailsRef.current;
    if (connected && continuous && details && parameter.normalizedValue !== null) {
      let gesture = gestures.current.get(parameter.index);
      if (!gesture || !gestureCurrent(gesture)) {
        gesture = {details, parameter, snapshot, gateway, observed: parameter.normalizedValue, sent: []};
        gestures.current.set(parameter.index, gesture);
      }
      previewQueue.current = {gesture, value, revision};
      void drainPreviews();
    }
    return revision;
  }, [connected, controller, drainPreviews, editor, gateway, gestureCurrent, reconcile, snapshot]);

  const commit = useCallback((parameter: BlockParameter, value: number) => {
    if (batchRunning.current) return;
    const revision = draft(parameter, value);
    const target = authorityTargets.current.get(parameter.index);
    if (target) controller.releaseInteraction(target, value);
    const existing = timers.current.get(parameter.index);
    if (existing !== undefined) window.clearTimeout(existing);
    timers.current.set(parameter.index, window.setTimeout(() => {
      timers.current.delete(parameter.index);
      if (revisions.current.get(parameter.index) !== revision) return;
      releases.current.set(parameter.index, {revision, apply: () => apply(parameter, value, revision)});
      void drainReleases();
    }, 8));
  }, [apply, controller, draft, drainReleases]);

  const commitBatch = useCallback(async (changes: Array<{ parameter: BlockParameter; value: number }>) => {
    if (snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block parameter edits are paused until the current-preset refresh succeeds.");
      return;
    }
    const details = detailsRef.current;
    if (!details || pending || releaseRunning.current || batchRunning.current || !changes.length) return;
    if (!connected) {
      editor.updateParameters(changes);
      reconcile({ ...snapshot, dirty: true });
      notice(`Preview: ${details.name} parameters adjusted.`);
      return;
    }
    // A linked control release owns its parameters until verification finishes.
    // Supersede their older timers, but let already admitted previews settle.
    batchRunning.current = true;
    const epoch = lifetime.current;
    const sameBlock = (block: BlockDetails | undefined) => block?.row === details.row
      && block.column === details.column && block.modelId === details.modelId && block.scene === details.scene;
    const current = () => epoch === lifetime.current && context.current.connected
      && samePresetAddress(context.current.snapshot, snapshot)
      && context.current.snapshot.presetIdentity === snapshot.presetIdentity
      && context.current.snapshot.activeScene === snapshot.activeScene && sameBlock(detailsRef.current);
    const assertCurrent = () => {if (!current()) throw new SupersededPresetOperation();};
    const batch = [...new Map(changes.map(change => [change.parameter.index, change])).values()];
    for (const {parameter, value} of batch) {
      const target = { kind: "parameter" as const, row: details.row, column: details.column, parameterIndex: parameter.index };
      authorityTargets.current.set(parameter.index, target);
      controller.updateInteraction(target, value);
      controller.releaseInteraction(target, value);
      const timer = timers.current.get(parameter.index);
      if (timer !== undefined) window.clearTimeout(timer);
      timers.current.delete(parameter.index);
      releases.current.delete(parameter.index);
    }
    setPending(true);
    notice("Applying parameter changes…");
    try {
      await waitForPreviews();
      assertCurrent();
      let currentBlock = await gateway.blockDetails(details.row, details.column, snapshot.presetName, requirePresetIdentity(snapshot));
      assertCurrent();
      if (!sameBlock(currentBlock)) throw new Error("The parameter block or scene changed. Select it again before editing.");
      // Validate the whole linked edit before its first write. A preview receipt
      // is only a possible value; the native cache must actually observe it.
      const expected = new Map<number, number>();
      for (const {parameter, value} of batch) {
        if (!Number.isFinite(value) || value < 0 || value > 1) throw new Error("Parameter values must be between 0 and 1.");
        const gesture = gestures.current.get(parameter.index);
        if (gesture) {
          observeGesture(gesture, currentBlock);
        } else {
          const observed = currentBlock.parameters.find(item => item.index === parameter.index);
          if (!observed?.writable || observed.normalizedValue == null || parameter.normalizedValue == null
              || !sameParameterValue(observed.normalizedValue, parameter.normalizedValue)) {
            throw new Error("A parameter changed before the linked edit. Refresh and retry.");
          }
        }
        expected.set(parameter.index, currentBlock.parameters.find(item => item.index === parameter.index)!.normalizedValue!);
      }
      let latestSnapshot: PresetSnapshot | undefined;
      let detail = "Parameters applied and verified.";
      for (const change of batch) {
        assertCurrent();
        const parameter = currentBlock.parameters.find((parameter) => parameter.index === change.parameter.index);
        if (!parameter?.writable || parameter.normalizedValue == null) throw new Error("A parameter is no longer writable.");
        const admittedValue = expected.get(parameter.index)!;
        if (!gestures.current.has(parameter.index) && sameParameterValue(admittedValue, change.value)) continue;
        const result = await gateway.setParameter(currentBlock.row, currentBlock.column, parameter.index, change.value, admittedValue, snapshot.activeScene, requireModelId(details), snapshot.presetName, requirePresetIdentity(snapshot));
        assertCurrent();
        const confirmed = result.block.parameters.find(item => item.index === parameter.index)?.normalizedValue;
        if (!sameBlock(result.block) || confirmed == null || !sameParameterValue(confirmed, change.value)) {
          throw new Error("A parameter changed before linked edit confirmation. Refresh and retry.");
        }
        currentBlock = result.block;
        latestSnapshot = result.snapshot ?? latestSnapshot;
        detail = result.detail;
      }
      if (batch.some(({parameter, value}) => {
        const confirmed = currentBlock.parameters.find(item => item.index === parameter.index)?.normalizedValue;
        return confirmed == null || !sameParameterValue(confirmed, value);
      })) throw new Error("A linked parameter changed while applying the batch. Refresh and retry.");
      editor.load(currentBlock);
      detailsRef.current = currentBlock;
      if (latestSnapshot) reconcile(latestSnapshot);
      notice(detail);
    } catch (error) {
      if (current()) {
        fail(error);
        try {
          const observed = await gateway.blockDetails(details.row, details.column, snapshot.presetName, requirePresetIdentity(snapshot));
          if (current() && sameBlock(observed)) {detailsRef.current = observed; editor.load(observed);}
        } catch { /* Recovery reads once and never replays a partial batch. */ }
      }
    } finally {
      if (epoch === lifetime.current) for (const {parameter} of batch) {
        targets.current.delete(parameter.index);
        gestures.current.delete(parameter.index);
      }
      batchRunning.current = false;
      setPending(false);
      void drainReleases();
    }
  }, [connected, controller, drainReleases, editor, fail, gateway, notice, observeGesture, pending, reconcile, setPending, snapshot, waitForPreviews]);

  const targetValue = useCallback((parameter: BlockParameter) => targets.current.get(parameter.index) ?? editor.drafts[parameter.index] ?? parameter.normalizedValue ?? 0, [editor.drafts]);
  const toggleSceneMode = useCallback(async (parameter: BlockParameter) => {
    if (snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block parameter edits are paused until the current-preset refresh succeeds.");
      return;
    }
    const details = detailsRef.current;
    if (!details || pending) return;
    if (!connected) {
      notice("Connect the Quad Cortex before changing a parameter's Scene assignment.");
      return;
    }
    setPending(true);
    notice(`${parameter.sceneMode ? "Removing" : "Assigning"} ${parameter.name} ${parameter.sceneMode ? "from" : "to"} Scenes…`);
    try {
      await waitForPreviews();
      const observed = await gateway.blockDetails(details.row, details.column, snapshot.presetName, requirePresetIdentity(snapshot));
      if (observed.modelId !== details.modelId || observed.scene !== details.scene) throw new Error("The parameter block or scene changed. Select it again before editing.");
      const current = observed.parameters.find((candidate) => candidate.index === parameter.index);
      if (!current || current.sceneMode !== parameter.sceneMode) throw new Error("The parameter's Scene assignment changed. Refresh and retry.");
      const enabled = !current.sceneMode;
      const result = await gateway.setParameterSceneMode(observed.row, observed.column, current.index, enabled, requireModelId(observed), snapshot.presetName, requirePresetIdentity(snapshot));
      const confirmed = await gateway.blockDetails(observed.row, observed.column, snapshot.presetName, requirePresetIdentity(snapshot));
      const confirmedParameter = confirmed.parameters.find((candidate) => candidate.index === current.index);
      if (confirmedParameter?.sceneMode !== enabled) throw new Error("The Quad Cortex did not confirm the parameter's Scene assignment.");
      detailsRef.current = confirmed;
      editor.load(confirmed);
      if (result.snapshot) reconcile(result.snapshot);
      recordHistory({
        label: `${observed.name} ${current.name} Scene assignment`,
        execute: (latest) => gateway.setParameterSceneMode(observed.row, observed.column, current.index, current.sceneMode, requireModelId(observed), latest.presetName, requirePresetIdentity(latest)),
        redo: (latest) => gateway.setParameterSceneMode(observed.row, observed.column, current.index, enabled, requireModelId(observed), latest.presetName, requirePresetIdentity(latest))
      });
      notice(result.detail);
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [connected, editor, fail, gateway, notice, pending, reconcile, recordHistory, setPending, snapshot, waitForPreviews]);
  const resetDefaults = useCallback(async () => {
    if (snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block parameter edits are paused until the current-preset refresh succeeds.");
      return;
    }
    const details = detailsRef.current;
    if (!details || pending) return;
    if (!connected) {
      notice("Connect the Quad Cortex before resetting block parameters.");
      return;
    }
    setPending(true);
    notice(`Resetting ${details.name} parameters…`);
    try {
      await waitForPreviews();
      const observed = await gateway.blockDetails(details.row, details.column, snapshot.presetName, requirePresetIdentity(snapshot));
      if (observed.modelId !== details.modelId || observed.scene !== details.scene) {
        throw new Error("The parameter block or scene changed. Select it again before resetting.");
      }
      const result = await gateway.resetBlockParameters(observed.row, observed.column, requireModelId(observed), snapshot.presetName, requirePresetIdentity(snapshot));
      const confirmed = await gateway.blockDetails(observed.row, observed.column, snapshot.presetName, requirePresetIdentity(snapshot));
      if (confirmed.modelId !== observed.modelId || confirmed.scene !== observed.scene) {
        throw new Error("The parameter block or scene changed before reset readback.");
      }
      detailsRef.current = confirmed;
      editor.load(confirmed);
      reconcile(result.snapshot ?? { ...snapshot, dirty: true });
      notice("Block parameters reset to defaults.");
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [connected, editor, fail, gateway, notice, pending, reconcile, setPending, snapshot, waitForPreviews]);
  const adjustEncoder = useCallback((
    role: string,
    delta: number,
    report: (message: string) => void,
    announceCommit = true,
  ) => {
    const details = detailsRef.current;
    if (!details) return false;
    const slot = PARAMETER_ENCODER_ROLES.indexOf(role as (typeof PARAMETER_ENCODER_ROLES)[number]);
    if (slot < 0) return false;
    const parameter = parameterEditorControlSlots(
      details.parameters.filter((candidate) => candidate.normalizedValue !== null),
      details.category,
      editor.page,
      parameterEditorPageSize(details.category, details.parameters),
    )[slot];
    if (!parameter) {
      report(`${role} is not assigned on this parameter page.`);
      return true;
    }
    if (!parameter.writable) {
      report(`${details.name} · ${parameter.name} is read-only.`);
      return true;
    }
    const value = Math.max(0, Math.min(1, targetValue(parameter) + Math.sign(delta) * parameterStep(parameter)));
    commit(parameter, value);
    if (announceCommit) report(`${details.name} · ${parameter.name}`);
    return true;
  }, [commit, editor.page, targetValue]);
  const hasPendingChanges = useCallback(() => batchRunning.current || releaseRunning.current || timers.current.size > 0 || targets.current.size > 0, []);

  return { draft, commit, commitBatch, applyResolvedParameter, toggleSceneMode, resetDefaults, adjustEncoder, cancel, targetValue, hasPendingChanges, updateDetails: (details: typeof editor.details) => { detailsRef.current = details; if (details) editor.load(details); } };
}
