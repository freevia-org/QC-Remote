import { requireModelId, requirePresetIdentity } from "@qc-remote/client";
import { SupersededPresetOperation } from "@qc-remote/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLatestValueWriter } from "./latest-value-writer.ts";
import type { BlockDetails, GridBlock, LaneControl } from "@qc-remote/client";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";
import type { ExpressionAssignmentAddress, ExpressionAssignmentState, ExpressionAssignmentTarget } from "./expression-assignment";

export interface ExpressionAssignmentOptions extends DeviceMutationWorkflowOptions {
  onClosePanel?(): void;
  sessionKey?: unknown;
  captureSessionCurrent?(): () => boolean;
}

const BLOCK_ADDRESS: ExpressionAssignmentAddress = { kind: "block" };

type ExpressionRange = { minimum: number; maximum: number };

/** One range slider's write slot: the address, parameter and pedal it trims. */
function rangeKey(address: ExpressionAssignmentAddress, details: BlockDetails, index: number, pedal: 1 | 2) {
  return `${address.kind === "lane" ? `lane:${address.control}` : "block"}:${details.row}:${details.column}:${index}:${pedal}`;
}

/**
 * Live Expression 1/2 pedal assignment for the selected block, against real
 * `BlockDetails`. Assign, trim and unassign for a real parameter, a lane
 * control, or the block's own bypass - pedal 0 clears any of the three,
 * echoed back by an authoritative readback rather than asserted by the host.
 * The bypass clear is less precise than a parameter's: `bypass_expression`'s
 * pedal is a plain scalar, erased on the wire by a zero rather than
 * transmitted as one, so a clear can only be confirmed as "the assignment
 * stops being reported," without distinguishing a dropped assignment from a
 * stored zero-pedal one.
 *
 * A lane control (Input Gate/Lane Output) is addressed by `(row, control)`
 * through `laneControlDetails`/`setLaneControlExpression` instead of a Grid
 * block's `(row, column)` through `blockDetails`/`setParameterExpression` - a
 * lane control has no `bypassExpression` at all (there is no separate
 * device-level bypass write for one; its own BYPASS is just parameter index 1,
 * same as any other assignable parameter), so it never gets a bypass tile and
 * `onUnassign`/`onAssign` never reach `setExpressionBypass` for one.
 */
export function useExpressionAssignment(options: ExpressionAssignmentOptions) {
  const { snapshot, connected, pending, setPending, onClosePanel } = options;
  const [, replaceMountedOwner] = useState(0);
  const latest = useRef(options);
  latest.current = options;
  const ownerRef = useRef<{ key: unknown; gateway: typeof options.gateway; connected: boolean; session: string | undefined; identity: string | undefined; snapshot: typeof snapshot; sessionCurrent(): boolean; current(): boolean } | null>(null);
  let owner = ownerRef.current;
  const sessionCurrent = options.captureSessionCurrent?.() ?? (() => true);
  if (!owner || (!owner.current() && sessionCurrent()) || owner.key !== options.sessionKey || owner.gateway !== options.gateway
      || owner.connected !== connected || owner.session !== snapshot.stateVersion?.session || owner.identity !== snapshot.presetIdentity) {
    const replacement = { key: options.sessionKey, gateway: options.gateway, connected, session: snapshot.stateVersion?.session,
      identity: snapshot.presetIdentity, snapshot, sessionCurrent, current: () => false };
    replacement.current = () => ownerRef.current === replacement && sessionCurrent();
    ownerRef.current = replacement;
    owner = replacement;
  }
  const held = owner;
  held.snapshot = snapshot;
  const guard = () => { if (!held.current()) throw new SupersededPresetOperation(); };
  // A host gateway can be a stable proxy whose underlying transport changes.
  // Validate the captured session both before dispatch and after its reply.
  const gateway = useMemo(() => new Proxy(options.gateway, {
    get(target, key) {
      const method = Reflect.get(target, key);
      if (typeof method !== "function") return method;
      return async (...args: unknown[]) => {
        guard();
        const result = await method.apply(target, args);
        guard();
        return result;
      };
    }
  }), [held, options.gateway]);
  // Undo entries outlive individual edits; bind them to the originating
  // device session, while the history workflow supplies the current preset.
  const historyGateway = useMemo(() => new Proxy(options.gateway, {
    get(target, key) {
      const method = Reflect.get(target, key);
      if (typeof method !== "function") return method;
      return async (...args: unknown[]) => {
        const current = () => held.sessionCurrent() && latest.current.sessionKey === held.key
          && latest.current.gateway === held.gateway && latest.current.connected
          && latest.current.snapshot.stateVersion?.session === held.session;
        if (!current()) throw new SupersededPresetOperation();
        const result = await method.apply(target, args);
        if (!current()) throw new SupersededPresetOperation();
        return result;
      };
    }
  }), [held, options.gateway]);
  const notice = (message: string) => { if (held.current()) latest.current.notice(message); };
  const fail = (error: unknown) => { if (held.current() && !(error instanceof SupersededPresetOperation)) latest.current.fail(error); };
  const reconcile = (incoming: typeof snapshot) => {
    if (!held.current()) return;
    // Keep a legitimate reply's new preset identity in this operation's owner.
    held.identity = incoming.presetIdentity;
    held.snapshot = incoming;
    latest.current.reconcile(incoming);
  };
  const recordHistory: typeof options.recordHistory = (entry) => { if (held.current()) latest.current.recordHistory(entry); };
  const [open, setOpen] = useState(false);
  const [address, setAddress] = useState<ExpressionAssignmentAddress>(BLOCK_ADDRESS);
  const [pedal, setPedal] = useState<1 | 2>(1);
  const [selected, setSelected] = useState<ExpressionAssignmentTarget>("bypass");
  // MIN/MAX RANGE drags write through the shared latest-value writer. Block
  // details are not re-read after a range write, so an accepted range is kept
  // against the details it was written over until those details are replaced.
  const physicalSession = useMemo(() => ({ gateway: options.gateway, key: options.sessionKey, connected, session: snapshot.stateVersion?.session }),
    [options.gateway, options.sessionKey, connected, snapshot.stateVersion?.session]);
  const ranges = useLatestValueWriter<ExpressionRange>({ pending, setPending, resetKey: held, commandSessionKey: options.commandSessionKey ?? physicalSession });
  const [written, setWritten] = useState<ReadonlyMap<string, { details: BlockDetails; range: ExpressionRange }>>(() => new Map());

  useEffect(() => {
    setOpen(false);
    setWritten(new Map());
  }, [held]);
  useEffect(() => {
    // StrictMode replays mount effects. Replace the revoked owner explicitly;
    // restoring it would revive reads started before that cleanup.
    if (!ownerRef.current) replaceMountedOwner((version) => version + 1);
    return () => { ownerRef.current = null; };
  }, []);

  const openAssignment = useCallback((details: BlockDetails, target: ExpressionAssignmentAddress = BLOCK_ADDRESS) => {
    if (!held.current()) return;
    setPedal(1);
    setAddress(target);
    const firstAssignable = details.parameters.find((parameter) => parameter.expressionAssignable && parameter.writable)?.index;
    setSelected(target.kind === "block" && details.bypassExpression ? "bypass" : firstAssignable ?? (target.kind === "block" ? "bypass" : 0));
    setOpen(true);
  }, [held]);

  const close = useCallback(() => { if (held.current()) setOpen(false); }, [held]);

  const findBlock = useCallback((details: BlockDetails): GridBlock | undefined =>
    snapshot.blocks.find((candidate) => candidate.row === details.row && candidate.column === details.column), [snapshot.blocks]);

  const assign = useCallback((details: BlockDetails, target: ExpressionAssignmentTarget) => {
    if (!held.current()) return;
    if (address.kind === "lane") {
      if (target === "bypass") return; // Unreachable from the UI - see the module comment - but never wire it if it is.
      if (!connected) { notice("Connect the Quad Cortex before assigning an expression pedal."); return; }
      const parameter = details.parameters.find((candidate) => candidate.index === target);
      if (!parameter) return;
      ranges.runCommand(async () => {
        const minimum = parameter.expressionMinimum ?? 0;
        const maximum = parameter.expressionMaximum ?? 1;
        notice(`Assigning ${parameter.name} to Expression ${pedal}…`);
        const result = await gateway.setLaneControlExpression(details.row, address.control, target, pedal, minimum, maximum, held.snapshot.presetName, requirePresetIdentity(held.snapshot));
        if (result.snapshot) reconcile(result.snapshot);
        notice(result.detail);
      }, fail);
      return;
    }
    const block = findBlock(details);
    if (!connected || !block?.modelId) { notice(!connected ? "Connect the Quad Cortex before assigning an expression pedal." : "Refresh the live Grid before assigning this block."); return; }
    ranges.runCommand(async () => {
      if (target === "bypass") {
        const before = details.bypassExpression;
        notice(`Assigning ${details.name}'s bypass to Expression ${pedal}…`);
        const result = await gateway.setExpressionBypass(details.row, details.column, pedal, (before?.mode ?? 0) as 0 | 1 | 2, before?.invert ?? false, before?.delayMs ?? 0, before?.latchEmulation ?? false, requireModelId(block), held.snapshot.presetName, requirePresetIdentity(held.snapshot));
        if (result.snapshot) reconcile(result.snapshot);
        recordHistory({ label: `${details.name} bypass expression`, execute: (current) => historyGateway.setExpressionBypass(details.row, details.column, pedal, (before?.mode ?? 0) as 0 | 1 | 2, before?.invert ?? false, before?.delayMs ?? 0, before?.latchEmulation ?? false, requireModelId(block), current.presetName, requirePresetIdentity(current)), redo: (current) => historyGateway.setExpressionBypass(details.row, details.column, pedal, (before?.mode ?? 0) as 0 | 1 | 2, before?.invert ?? false, before?.delayMs ?? 0, before?.latchEmulation ?? false, requireModelId(block), current.presetName, requirePresetIdentity(current)) });
        notice(result.detail);
      } else {
        const parameter = details.parameters.find((candidate) => candidate.index === target);
        if (!parameter) return;
        const minimum = parameter.expressionMinimum ?? 0;
        const maximum = parameter.expressionMaximum ?? 1;
        notice(`Assigning ${parameter.name} to Expression ${pedal}…`);
        const result = await gateway.setParameterExpression(details.row, details.column, target, pedal, minimum, maximum, requireModelId(block), held.snapshot.presetName, requirePresetIdentity(held.snapshot));
        if (result.snapshot) reconcile(result.snapshot);
        notice(result.detail);
      }
    }, fail);
  }, [address, connected, fail, findBlock, gateway, historyGateway, notice, pedal, ranges.runCommand, reconcile, recordHistory, snapshot]);

  const setRange = useCallback((details: BlockDetails, index: number, minimum: number, maximum: number) => {
    if (!held.current() || !connected) return;
    const target = address;
    const block = target.kind === "lane" ? undefined : findBlock(details);
    if (target.kind !== "lane" && !block?.modelId) return;
    const key = rangeKey(target, details, index, pedal);
    ranges.submit(key, { minimum, maximum }, {
      write: (range) => target.kind === "lane"
        ? gateway.setLaneControlExpression(details.row, target.control, index, pedal, range.minimum, range.maximum, held.snapshot.presetName, requirePresetIdentity(held.snapshot))
        : gateway.setParameterExpression(details.row, details.column, index, pedal, range.minimum, range.maximum, requireModelId(block!), held.snapshot.presetName, requirePresetIdentity(held.snapshot)),
      accept: (result, range) => {
        if (result.snapshot) reconcile(result.snapshot);
        setWritten((previous) => new Map(previous).set(key, { details, range }));
      },
      error: fail
    });
  }, [address, connected, fail, findBlock, gateway, pedal, ranges.submit, reconcile, snapshot]);

  const unassign = useCallback((details: BlockDetails, target: ExpressionAssignmentTarget) => {
    if (!held.current()) return;
    if (target === "bypass") {
      if (address.kind === "lane") return; // Unreachable from the UI - a lane control has no bypass tile - but never wire it if it is.
      const block = findBlock(details);
      if (!connected || !block?.modelId) { notice(!connected ? "Connect the Quad Cortex before clearing an expression assignment." : "Refresh the live Grid before clearing this assignment."); return; }
      const before = details.bypassExpression;
      ranges.runCommand(async () => {
        notice(`Clearing ${details.name}'s bypass expression assignment…`);
        try {
        // Matches the required clear exactly: pedal 0. Unlike a
        // parameter's clear, this is not free to zero the rest: commands.rs's
        // set_expression_bypass_target always sends two frames, an Expression
        // carrying the pedal and a separate ExpressionBypassInfo carrying
        // mode/invert/delayMs/latchEmulation, with no pedal-0 branch that skips
        // the second one - so a clear reaches the device with whatever those
        // four values are set to. Threading them through from the existing
        // assignment, exactly as the assign call above threads `before`
        // through its own write, is what leaves the block's bypass info as it
        // was rather than overwriting it with STOP/false/0/false.
        const result = await gateway.setExpressionBypass(details.row, details.column, 0, (before?.mode ?? 0) as 0 | 1 | 2, before?.invert ?? false, before?.delayMs ?? 0, before?.latchEmulation ?? false, requireModelId(block), held.snapshot.presetName, requirePresetIdentity(held.snapshot));
        if (result.snapshot) reconcile(result.snapshot);
        notice(result.detail);
        } catch (error) { fail(error); }
      }, fail);
      return;
    }
    const index = target;
    const parameter = details.parameters.find((candidate) => candidate.index === index);
    if (!parameter) return;
    if (address.kind === "lane") {
      if (!connected) { notice("Connect the Quad Cortex before clearing an expression assignment."); return; }
      ranges.runCommand(async () => {
        notice(`Clearing ${parameter.name}'s expression assignment…`);
        try {
        // Matches the required clear exactly: pedal 0, range reset to
        // 0/1 - not "pedal 0 with the existing range".
        const result = await gateway.setLaneControlExpression(details.row, address.control, index, 0, 0, 1, held.snapshot.presetName, requirePresetIdentity(held.snapshot));
        if (result.snapshot) reconcile(result.snapshot);
        notice(result.detail);
        } catch (error) { fail(error); }
      }, fail);
      return;
    }
    const block = findBlock(details);
    if (!connected || !block?.modelId) { notice(!connected ? "Connect the Quad Cortex before clearing an expression assignment." : "Refresh the live Grid before clearing this assignment."); return; }
    ranges.runCommand(async () => {
      notice(`Clearing ${parameter.name}'s expression assignment…`);
      try {
      // Matches the required clear exactly: pedal 0, range reset to
      // 0/1 - not "pedal 0 with the existing range".
      const result = await gateway.setParameterExpression(details.row, details.column, index, 0, 0, 1, requireModelId(block), held.snapshot.presetName, requirePresetIdentity(held.snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      notice(result.detail);
      } catch (error) { fail(error); }
    }, fail);
  }, [address, connected, fail, findBlock, gateway, notice, ranges.runCommand, reconcile, snapshot]);

  const assignmentFor = useCallback((details: BlockDetails): ExpressionAssignmentState | undefined => {
    if (!held.current() || !open) return undefined;
    // The slider being dragged, or the range last written over these details,
    // is what the panel shows; the other bound of a drag comes from it too.
    let shown = details;
    for (const parameter of details.parameters) {
      const key = rangeKey(address, details, parameter.index, pedal);
      const kept = written.get(key);
      const range = ranges.drafts.get(key) ?? (kept?.details === details ? kept.range : undefined);
      if (!range) continue;
      if (shown === details) shown = { ...details, parameters: [...details.parameters] };
      shown.parameters[shown.parameters.indexOf(parameter)] = { ...parameter, expressionMinimum: range.minimum, expressionMaximum: range.maximum };
    }
    return {
      details: shown,
      address,
      pedal,
      onPedalChange: (value) => { if (held.current()) setPedal(value); },
      selected,
      onSelect: (value) => { if (held.current()) setSelected(value); },
      // A range slider stays live while its own value is being written.
      pending: pending && !ranges.active,
      onAssign: (target) => void assign(details, target),
      onSetRange: (index, minimum, maximum) => void setRange(details, index, minimum, maximum),
      onUnassign: (target) => void unassign(details, target),
      onClose: () => { if (held.current()) { close(); onClosePanel?.(); } }
    };
  }, [address, assign, close, onClosePanel, open, pedal, pending, ranges.active, ranges.drafts, selected, setRange, unassign, written]);

  return { openAssignment, close, assignmentFor, isOpen: open };
}

// Re-exported so a caller building an address does not need a second import.
export type { LaneControl };
