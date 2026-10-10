import { requireModelId, requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { BlockDetails, DeviceActionResult, GatewayTransport, GridBlock, LaneControl, LibraryEntry, LibraryFolder, ModelEntry, PresetSnapshot } from "@qc-remote/client";
import { demoBlockDetails, laneControlSelectionId, parseLaneControlSelectionId, sceneLetter } from "@qc-remote/core";
import { deviceBrowserMatches } from "./add-device-catalog.ts";
import { closeGridBlockOnDevice, openGridBlockOnDevice } from "./device-editor-mirroring.ts";
import type { CorOsAddDevicePickerState } from "./add-device-picker";
import type { BlockEditorSessionController } from "./use-block-editor-session";
import type { DeviceHistoryEntry } from "./use-device-history";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";

export interface GridWorkflowOptions extends DeviceMutationWorkflowOptions {
  topologyComplete?: boolean;
  selectedBlockId: string;
  setSelectedBlockId: Dispatch<SetStateAction<string>>;
  editor: BlockEditorSessionController;
  closePresetDirectory?(): void;
  onOpenManagement?(): void;
  onOpenAdd?(): void;
  /**
   * Open Add Device as the unit's own device browser over the live Grid
   * (`addPickerProps`) instead of calling `onOpenAdd` for a host panel.
   */
  liveAddDevicePicker?: boolean;
  onClosePanel?(): void;
  /** Increment when CorOS announces that the installed-model catalog changed. */
  catalogRevision?: number;
}

function assertBlockLocation(details: BlockDetails, block: Pick<GridBlock, "row" | "column">, scene: number) {
  if (details.row !== block.row || details.column !== block.column || details.scene !== scene) {
    throw new Error("The block or scene changed. Refresh the Grid and select the block again.");
  }
}

function assertBlockContext(details: BlockDetails, block: Pick<GridBlock, "row" | "column" | "modelId">, scene: number) {
  assertBlockLocation(details, block, scene);
  if (details.modelId !== block.modelId) throw new Error("The block or scene changed. Refresh the Grid and select the block again.");
}

/** Shared Grid block catalog, selection, editor, clipboard, and mutation workflow. */
export function useGridWorkflow(options: GridWorkflowOptions) {
  const {
    gateway, snapshot, selectedBlockId, setSelectedBlockId, editor, pending, setPending,
    reconcile, recordHistory, notice, fail, closePresetDirectory, onOpenManagement, onOpenAdd, liveAddDevicePicker, onClosePanel
  } = options;
  const deviceConnected = options.connected;
  const topologyComplete = options.topologyComplete ?? true;
  const connected = deviceConnected && topologyComplete;
  const [moveDestination, setMoveDestination] = useState<number>();
  /** The picker's destination row; undefined means the block's own row. */
  const [moveDestinationRow, setMoveDestinationRow] = useState<number>();
  const [footswitchDraft, setFootswitchDraft] = useState<number | null>(null);
  const [footswitchAssignmentPending, setFootswitchAssignmentPending] = useState(false);
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [captures, setCaptures] = useState<LibraryEntry[]>([]);
  const [captureFolders, setCaptureFolders] = useState<LibraryFolder[]>([]);
  const [captureError, setCaptureError] = useState<string>();
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelFilter, setModelFilter] = useState("");
  const [addCell, setAddCell] = useState("");
  const [addModelId, setAddModelId] = useState<number>();
  const [pinnedModelIds, setPinnedModelIds] = useState<number[]>([]);
  /** The live device browser is open over the Grid for `addCell`. */
  const [addPickerOpen, setAddPickerOpen] = useState(false);
  const [replaceTarget, setReplaceTarget] = useState<GridBlock>();
  /** The browser's active rail category; undefined lists every category. */
  const [addCategory, setAddCategory] = useState<string>();
  const [detailsLoading, setDetailsLoading] = useState(false);
  const [clipboard, setClipboard] = useState<BlockDetails>();
  const request = useRef(0);
  const copyRequest = useRef(0);
  const details = editor.details;
  const currentDetails = useRef(details);
  currentDetails.current = details;
  useEffect(() => {
    setModels([]);
    setAddModelId(undefined);
  }, [options.catalogRevision]);
  // Reads belong to the originating device context, including scene changes.
  // Closing/opening a panel also advances request synchronously below.
  useEffect(() => () => {
    request.current++;
    copyRequest.current++;
  }, [connected, snapshot.presetIdentity, snapshot.setlistKey, snapshot.presetPosition,
    snapshot.presetName, snapshot.activeScene]);
  const sceneBindings = useRef({gateway, editor, notice, fail});
  sceneBindings.current = {gateway, editor, notice, fail};

  const filteredModels = useMemo(() => {
    return modelFilter.trim() ? models.filter((model) => deviceBrowserMatches(model, modelFilter)) : models;
  }, [modelFilter, models]);

  const close = useCallback(() => {
    request.current += 1;
    copyRequest.current += 1;
    setSelectedBlockId("");
    editor.close();
    setDetailsLoading(false);
    setMoveDestination(undefined);
    setMoveDestinationRow(undefined);
    setFootswitchDraft(null);
    setFootswitchAssignmentPending(false);
    onClosePanel?.();
    notice("Parameter editor closed.");
  }, [editor, notice, onClosePanel, setSelectedBlockId]);

  const closeOnDevice = useCallback(async () => {
    close();
    if (!connected) return;
    const sequence = request.current;
    try { await closeGridBlockOnDevice(gateway, () => sequence === request.current); }
    catch (error) { fail(error); }
  }, [close, connected, fail, gateway]);

  const openBlock = useCallback(async (block: GridBlock) => {
    if (!topologyComplete || snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block editing is paused until the current preset refresh succeeds.");
      return;
    }
    if (pending) { notice("A device command is already in progress."); return; }
    const sequence = ++request.current;
    closePresetDirectory?.();
    setFootswitchAssignmentPending(false);
    setMoveDestination(undefined);
    setMoveDestinationRow(undefined);
    if (!connected) {
      const preview = demoBlockDetails(block, snapshot.activeScene);
      setSelectedBlockId(block.id);
      setFootswitchDraft(block.footswitch ?? null);
      editor.load(preview, true);
      notice(`Preview: ${block.name} parameter editor opened with the ${preview.category} control layout.`);
      return;
    }
    setDetailsLoading(true);
    notice(`Reading ${block.name} parameters…`);
    try {
      const next = await gateway.blockDetails(block.row, block.column, snapshot.presetName, requirePresetIdentity(snapshot));
      if (sequence !== request.current) return;
      assertBlockContext(next, block, snapshot.activeScene);
      setSelectedBlockId(block.id);
      setFootswitchDraft(block.footswitch ?? null);
      editor.load(next, true);
      notice(`${next.name} parameters synchronized.`);
      // Keep the app responsive. The QC mirror is a separate, guarded
      // transaction because its one-shot graphics-tree safety read can be
      // slower than the already-synchronized parameter read.
      void openGridBlockOnDevice(gateway, block, () => sequence === request.current)
        .catch((error) => { if (sequence === request.current) fail(error); });
    } catch (error) { if (sequence === request.current) fail(error); }
    finally { if (sequence === request.current) setDetailsLoading(false); }
  }, [closePresetDirectory, connected, editor, fail, gateway, notice, pending, setSelectedBlockId, snapshot.activeScene, snapshot.gridLayoutComplete, snapshot.presetIdentity, snapshot.presetName, topologyComplete]);

  const openRoutingNode = useCallback(async (row: number, node: "splitter" | "mixer") => {
    if (!connected) { notice(`Connect the Quad Cortex to read the live ${node === "splitter" ? "Splitter" : "Mixer"} parameters.`); return; }
    if (pending) { notice("A device command is already in progress."); return; }
    const sequence = ++request.current;
    const column = node === "splitter" ? 8 : 9;
    const name = node === "splitter" ? "Splitter" : "Mixer";
    closePresetDirectory?.();
    setFootswitchAssignmentPending(false);
    setMoveDestination(undefined);
    setMoveDestinationRow(undefined);
    setDetailsLoading(true);
    notice(`Reading ${name} parameters…`);
    try {
      const next = await gateway.blockDetails(row, column, snapshot.presetName, requirePresetIdentity(snapshot));
      if (sequence !== request.current) return;
      assertBlockLocation(next, {row, column}, snapshot.activeScene);
      setSelectedBlockId(`routing-${row}-${node}`);
      setFootswitchDraft(null);
      editor.load(next, true);
      notice(`${name} parameters synchronized.`);
    } catch (error) { if (sequence === request.current) fail(error); }
    finally { if (sequence === request.current) setDetailsLoading(false); }
  }, [closePresetDirectory, connected, editor, fail, gateway, notice, pending, setSelectedBlockId, snapshot.activeScene, snapshot.presetIdentity, snapshot.presetName]);

  /**
   * Input Gate and Lane Output: real `BlockDetails`, real expression assignment,
   * but never a `(row, column)` Grid block - `laneControlDetails`/
   * `setLaneControlExpression` address them by `(row, control)` instead. Follows
   * `openRoutingNode`'s own shape exactly (no demo-mode preview branch either;
   * both require a connected gateway, real or virtual), with `selectedBlockId`
   * carrying `lane-${row}-${control}` so the scene-resync effect below and the
   * parameter-editor/lane-control-panel bindings can tell which kind of thing
   * `editor.details` currently holds.
   */
  const openLaneControl = useCallback(async (row: number, control: LaneControl) => {
    const name = control === "inputGate" ? "Input Gate" : "Lane Output";
    if (!connected) { notice(`Connect the Quad Cortex to read the live ${name} parameters.`); return; }
    if (pending) { notice("A device command is already in progress."); return; }
    const sequence = ++request.current;
    closePresetDirectory?.();
    setFootswitchAssignmentPending(false);
    setMoveDestination(undefined);
    setMoveDestinationRow(undefined);
    setDetailsLoading(true);
    notice(`Reading ${name} parameters…`);
    try {
      const next = await gateway.laneControlDetails(row, control, snapshot.presetName, requirePresetIdentity(snapshot));
      if (sequence !== request.current) return;
      if (next.row !== row) throw new Error("The block or scene changed. Refresh the Grid and select the block again.");
      setSelectedBlockId(laneControlSelectionId(row, control));
      setFootswitchDraft(null);
      editor.load(next, true);
      notice(`${name} parameters synchronized.`);
    } catch (error) { if (sequence === request.current) fail(error); }
    finally { if (sequence === request.current) setDetailsLoading(false); }
  }, [closePresetDirectory, connected, editor, fail, gateway, notice, pending, setSelectedBlockId, snapshot.activeScene, snapshot.presetIdentity, snapshot.presetName]);

  useEffect(() => {
    if (!details || details.scene === snapshot.activeScene) { setDetailsLoading(false); return; }
    // The host recreates these bindings on renders. Only a changed editor or
    // device context can supersede this read, not its own loading-state update.
    const {gateway, editor, notice, fail} = sceneBindings.current;
    if (!connected) { setDetailsLoading(false); editor.setScene(snapshot.activeScene); return; }
    // A lane control has no `(row, column)` to re-read with `blockDetails` -
    // `selectedBlockId` is the only place that address survives a scene change,
    // since `details.column` for a lane control is not a real Grid column.
    const lane = parseLaneControlSelectionId(selectedBlockId);
    let cancelled = false;
    const sequence = request.current;
    const current = () => !cancelled && sequence === request.current;
    setDetailsLoading(true);
    const read = lane
      ? gateway.laneControlDetails(details.row, lane.control, snapshot.presetName, requirePresetIdentity(snapshot))
      : gateway.blockDetails(details.row, details.column, snapshot.presetName, requirePresetIdentity(snapshot));
    void read.then((next) => {
      if (current()) {
        if (lane) { if (next.row !== details.row) throw new Error("The block or scene changed. Refresh the Grid and select the block again."); }
        else assertBlockContext(next, details, snapshot.activeScene);
        editor.load(next);
        notice(`${next.name} synchronized for Scene ${sceneLetter(snapshot.activeScene)}.`);
      }
    }).catch((error) => { if (current()) fail(error); }).finally(() => { if (current()) setDetailsLoading(false); });
    return () => { cancelled = true; };
  }, [connected, details?.column, details?.row, details?.modelId, details?.scene, selectedBlockId,
    snapshot.activeScene, snapshot.setlistKey, snapshot.presetPosition, snapshot.presetIdentity, snapshot.presetName]);

  /** Move `block` to any empty Grid cell. The picker passes a same-row column;
   * a Grid drag passes whichever cell the block was dropped on. Selection and
   * the open panel follow the block only when it was the one being edited. */
  const moveBlockTo = useCallback(async (block: GridBlock | undefined, toRow: number, toColumn: number, followSelection?: boolean) => {
    if (pending) return;
    if (!connected || !block?.modelId) { notice(!connected ? "Connect the Quad Cortex before moving a block." : "Refresh the live Grid before moving this block."); return; }
    if (toRow === block.row && toColumn === block.column) return;
    if (snapshot.blocks.some((candidate) => candidate.row === toRow && candidate.column === toColumn)) { notice(`Row ${toRow + 1}, column ${toColumn + 1} is occupied. Move ${block.name} to an empty cell.`); return; }
    setPending(true);
    notice(`Moving ${block.name}…`);
    try {
      const modelId = block.modelId;
      const result = await gateway.moveBlock(block.row, block.column, toColumn, modelId, toRow, snapshot.presetName, requirePresetIdentity(snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      const wasSelected = followSelection ?? selectedBlockId === block.id;
      if (wasSelected) setSelectedBlockId(`block-${toRow}-${toColumn}`);
      recordHistory({ label: `move ${block.name}`, execute: (current) => gateway.moveBlock(toRow, toColumn, block.column, modelId, block.row, current.presetName, requirePresetIdentity(current)), redo: (current) => gateway.moveBlock(block.row, block.column, toColumn, modelId, toRow, current.presetName, requirePresetIdentity(current)) });
      if (wasSelected) onClosePanel?.();
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [connected, fail, gateway, notice, onClosePanel, pending, reconcile, recordHistory, selectedBlockId, setPending, setSelectedBlockId, snapshot.blocks, snapshot.presetIdentity, snapshot.presetName]);

  const move = useCallback(async () => {
    if (!details || moveDestination === undefined || pending) return;
    const block = snapshot.blocks.find((candidate) => candidate.row === details.row && candidate.column === details.column);
    await moveBlockTo(block, moveDestinationRow ?? details.row, moveDestination, true);
  }, [details, moveBlockTo, moveDestination, moveDestinationRow, pending, snapshot.blocks]);

  const assignFootswitch = useCallback(async (requested = footswitchDraft) => {
    if (!details || pending) return;
    const block = snapshot.blocks.find((candidate) => candidate.row === details.row && candidate.column === details.column);
    if (!block || requested === (block.footswitch ?? null)) return;
    if (!connected || !block.modelId) { notice(!connected ? "Connect the Quad Cortex before changing a footswitch assignment." : "Refresh the live Grid before changing this assignment."); return; }
    const target = requested === null ? "unassign it from its STOMP footswitch" : `assign it to Footswitch ${sceneLetter(requested)}`;
    setPending(true);
    notice(`Updating ${block.name} footswitch assignment…`);
    try {
      const before = block.footswitch ?? null;
      const beforeType = block.footswitchAssignmentType ?? null;
      const assignmentType = beforeType ?? "primary";
      const result = await gateway.setBlockFootswitch(block.row, block.column, requested, assignmentType, before, beforeType, block.modelId, snapshot.presetName, requirePresetIdentity(snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      setFootswitchDraft(requested);
      recordHistory({ label: `${block.name} footswitch assignment`, execute: (current) => gateway.setBlockFootswitch(block.row, block.column, before, assignmentType, requested, requested === null ? null : assignmentType, block.modelId as number, current.presetName, requirePresetIdentity(current)), redo: (current) => gateway.setBlockFootswitch(block.row, block.column, requested, assignmentType, before, before === null ? null : beforeType, block.modelId as number, current.presetName, requirePresetIdentity(current)) });
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [connected, details, fail, footswitchDraft, gateway, notice, pending, reconcile, recordHistory, setPending, snapshot.blocks, snapshot.presetIdentity, snapshot.presetName]);

  /** Remove `block`, whether or not it is the selected one: a Grid drag can
   * drop any block off the Grid without first selecting it. */
  const removeBlock = useCallback(async (block: GridBlock | undefined) => {
    if (!block || block.column < 0 || !block.modelId || pending) return;
    if (!connected) { notice("Connect the Quad Cortex before removing a block."); return; }
    setPending(true);
    notice(`Removing ${block.name}…`);
    try {
      const removed = await gateway.blockDetails(block.row, block.column, snapshot.presetName, requirePresetIdentity(snapshot));
      const result = await gateway.removeBlock(block.row, block.column, block.modelId, snapshot.presetName, requirePresetIdentity(snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      const wasSelected = selectedBlockId === block.id;
      if (wasSelected) {
        setSelectedBlockId("");
        editor.close();
      }
      recordHistory({
        label: `remove ${block.name}`,
        execute: async (current) => {
          let latest = await gateway.addBlock(block.row, block.column, block.modelId as number, current.presetName, null, requirePresetIdentity(current));
          let working = latest.snapshot ?? current;
          const defaults = await gateway.blockDetails(block.row, block.column, working.presetName, requirePresetIdentity(working));
          for (const parameter of removed.parameters) {
            const initial = defaults.parameters.find((candidate) => candidate.index === parameter.index);
            if (!parameter.writable || parameter.normalizedValue === null || initial?.normalizedValue === null || initial?.normalizedValue === undefined || Math.abs(parameter.normalizedValue - initial.normalizedValue) < .000001) continue;
            latest = await gateway.setParameter(block.row, block.column, parameter.index, parameter.normalizedValue, initial.normalizedValue, snapshot.activeScene, requireModelId(removed), working.presetName, requirePresetIdentity(working));
            working = latest.snapshot ?? working;
          }
          const restored = working.blocks.find((candidate) => candidate.row === block.row && candidate.column === block.column);
          if ((block.footswitch ?? null) !== (restored?.footswitch ?? null)) {
            latest = await gateway.setBlockFootswitch(block.row, block.column, block.footswitch ?? null, block.footswitchAssignmentType ?? "primary", restored?.footswitch ?? null, restored?.footswitchAssignmentType ?? null, block.modelId as number, working.presetName, requirePresetIdentity(working));
            working = latest.snapshot ?? working;
          }
          const finalBlock = working.blocks.find((candidate) => candidate.row === block.row && candidate.column === block.column);
          if ((block.bypassed ?? false) !== (finalBlock?.bypassed ?? false)) latest = await gateway.toggleBypass(block.row, block.column, snapshot.activeScene, finalBlock?.bypassed ?? false, block.bypassed ?? false, requireModelId(block), working.presetName, requirePresetIdentity(working));
          return { ...latest, detail: `Restored ${block.name} and its previous settings.` };
        },
        redo: (current) => gateway.removeBlock(block.row, block.column, block.modelId as number, current.presetName, requirePresetIdentity(current))
      });
      if (wasSelected) onClosePanel?.();
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [connected, editor, fail, gateway, notice, onClosePanel, pending, reconcile, recordHistory, selectedBlockId, setPending, setSelectedBlockId, snapshot.activeScene, snapshot.presetIdentity, snapshot.presetName]);

  const remove = useCallback(() => removeBlock(snapshot.blocks.find((candidate) => candidate.id === selectedBlockId)), [removeBlock, selectedBlockId, snapshot.blocks]);

  /** `target` is the Grid cell the user pointed at; without one the first empty cell wins. */
  const openAdd = useCallback(async (target?: { row: number; column: number }) => {
    if (!connected || pending) { notice(!connected ? "Connect the Quad Cortex before adding a block." : "A device command is already in progress."); return; }
    const isEmpty = (row: number, column: number) => !snapshot.blocks.some((block) => block.row === row && block.column === column);
    const requested = target && isEmpty(target.row, target.column) ? `${target.row}:${target.column}` : undefined;
    const firstEmpty = requested ?? Array.from({ length: 32 }, (_, index) => `${Math.floor(index / 8)}:${index % 8}`).find((cell) => {
      const [row, column] = cell.split(":").map(Number);
      return isEmpty(row, column);
    });
    if (!firstEmpty) { notice("The Grid has no empty block cells."); return; }
    setAddCell(firstEmpty);
    setModelFilter("");
    setCaptureError(undefined);
    if (liveAddDevicePicker) {
      setAddCategory(undefined);
      setAddPickerOpen(true);
    } else onOpenAdd?.();
    // The host panel preselects its first model; the live browser starts with
    // nothing chosen, as the unit's does.
    if (models.length) setAddModelId(liveAddDevicePicker ? undefined : models[0].id);
    setModelsLoading(true);
    try {
      const [result, captureResult, pinnedResult] = await Promise.all([
        models.length ? Promise.resolve({ models }) : gateway.listModels(),
        gateway.captures().then((value) => ({ value }), (error) => ({ error })),
        gateway.pinnedModels().then((value) => ({ value }), (error) => ({ error }))
      ]);
      if (!models.length) setModels(result.models);
      if ("value" in captureResult) {
        setCaptures(captureResult.value.entries);
        setCaptureFolders(captureResult.value.folders ?? []);
      }
      else {
        setCaptures([]);
        setCaptureFolders([]);
        setCaptureError(captureResult.error instanceof Error ? captureResult.error.message : String(captureResult.error));
      }
      if ("value" in pinnedResult) setPinnedModelIds(pinnedResult.value.models);
      setAddModelId(liveAddDevicePicker ? undefined : result.models[0]?.id);
      if (!models.length) notice(`${result.models.length} installed block models synchronized.`);
    } catch (error) {
      setCaptureError(error instanceof Error ? error.message : String(error));
      fail(error);
    }
    finally { setModelsLoading(false); }
  }, [connected, fail, gateway, liveAddDevicePicker, models, notice, onOpenAdd, pending, snapshot.blocks]);

  const openReplace = useCallback(async () => {
    const target = snapshot.blocks.find((block) => block.id === selectedBlockId && block.column >= 0 && block.modelId !== undefined);
    if (!target) { notice("Select a Grid block before changing its device."); return; }
    if (!connected || pending) { notice(!connected ? "Connect the Quad Cortex before changing a device." : "A device command is already in progress."); return; }
    setReplaceTarget(target);
    setAddCell(`${target.row}:${target.column}`);
    setModelFilter(""); setCaptureError(undefined); setAddCategory(undefined); setAddPickerOpen(true); setModelsLoading(true);
    try {
      const [result, pinnedResult] = await Promise.all([
        models.length ? Promise.resolve({ models }) : gateway.listModels(),
        gateway.pinnedModels().then((value) => ({ value }), (error) => ({ error }))
      ]);
      if (!models.length) setModels(result.models);
      if ("value" in pinnedResult) setPinnedModelIds(pinnedResult.value.models);
      setAddModelId(undefined);
    } catch (error) { fail(error); }
    finally { setModelsLoading(false); }
  }, [connected, fail, gateway, models, notice, pending, selectedBlockId, snapshot.blocks]);

  const toggleModelPinned = useCallback(async (modelId: number) => {
    if (pending) return;
    const expectedPinned = pinnedModelIds.includes(modelId);
    const model = models.find((candidate) => candidate.id === modelId);
    setPending(true);
    notice(`${expectedPinned ? "Unpinning" : "Pinning"} ${model?.name ?? "device"}…`);
    try {
      const result = await gateway.setModelPinned(modelId, !expectedPinned, expectedPinned);
      const confirmed = await gateway.pinnedModels();
      if (confirmed.models.includes(modelId) === expectedPinned) throw new Error("The Quad Cortex did not confirm the pinned-device change.");
      setPinnedModelIds(confirmed.models);
      notice(result.detail);
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [fail, gateway, models, notice, pending, pinnedModelIds, setPending]);

  /** Place the chosen model. The host panel places its highlighted `addModelId`
   * from the filtered list; the live browser passes the row it was tapped on,
   * which it has already filtered itself. Both reach the same placement.
   *
   * No confirmation: placing a block destroys nothing, `recordHistory` below
   * makes it undoable, and it is temporary until the preset is saved. The DSP
   * warning the confirmation used to carry was a guess about what the QC would
   * say - if the preset has too little DSP the QC refuses and `fail` reports
   * the refusal. */
  const add = useCallback(async (chosenModelId?: number) => {
    const modelId = chosenModelId ?? addModelId;
    if (!addCell || modelId === undefined || pending) return;
    const [row, column] = addCell.split(":").map(Number);
    const model = (chosenModelId === undefined ? filteredModels : models).find((candidate) => candidate.id === modelId);
    if (!model) return;
    if (model.available === false) { notice(`${model.name} is unavailable on this QC.`); return; }
    setPending(true);
    notice(`Placing ${model.name}…`);
    try {
      const result = replaceTarget
        ? await gateway.replaceBlock(row, column, modelId, requireModelId(replaceTarget), snapshot.presetName, requirePresetIdentity(snapshot))
        : await gateway.addBlock(row, column, modelId, snapshot.presetName, null, requirePresetIdentity(snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      setSelectedBlockId(`block-${row}-${column}`);
      if (replaceTarget) {
        const previous = requireModelId(replaceTarget);
        recordHistory({ label: `change device to ${model.name}`, execute: (current) => gateway.replaceBlock(row, column, previous, modelId, current.presetName, requirePresetIdentity(current)), redo: (current) => gateway.replaceBlock(row, column, modelId, previous, current.presetName, requirePresetIdentity(current)) });
      } else recordHistory({ label: `add ${model.name}`, execute: (current) => gateway.removeBlock(row, column, modelId, current.presetName, requirePresetIdentity(current)), redo: (current) => gateway.addBlock(row, column, modelId, current.presetName, null, requirePresetIdentity(current)) });
      setAddPickerOpen(false);
      setReplaceTarget(undefined);
      onClosePanel?.();
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [addCell, addModelId, fail, filteredModels, gateway, models, notice, onClosePanel, pending, reconcile, recordHistory, replaceTarget, setPending, setSelectedBlockId, snapshot.presetIdentity, snapshot.presetName]);

  const closeAddPicker = useCallback(() => { setAddPickerOpen(false); setReplaceTarget(undefined); }, []);

  const addCapture = useCallback(async (capture: LibraryEntry) => {
    // ProductData.version is a protobuf string. Android/Windows serialize it as
    // "1"/"2", so accept that native shape as well as older numeric fixtures.
    const captureVersion = Number(capture.version);
    if (!addCell || pending || (captureVersion !== 1 && captureVersion !== 2)) return;
    const [row, column] = addCell.split(":").map(Number);
    const modelId = captureVersion === 2 ? 14001 : 14000;
    setPending(true);
    notice(`Loading Neural Capture ${capture.name}…`);
    try {
      const result = await gateway.loadCapture(row, column, capture.key, capture.name, modelId, null, snapshot.presetName, requirePresetIdentity(snapshot));
      if (result.snapshot) reconcile(result.snapshot);
      setSelectedBlockId(`block-${row}-${column}`);
      setAddPickerOpen(false);
      onClosePanel?.();
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [addCell, fail, gateway, notice, onClosePanel, pending, reconcile, setPending, setSelectedBlockId, snapshot.presetIdentity, snapshot.presetName]);

  /** The live device browser, threaded to the Grid like the routing picker. */
  const addPickerProps = useMemo<CorOsAddDevicePickerState | undefined>(() => {
    if (!addPickerOpen || !addCell) return undefined;
    const [row, column] = addCell.split(":").map(Number);
    return {
      row,
      column,
      models,
      captures: replaceTarget ? [] : captures,
      captureFolders: replaceTarget ? [] : captureFolders,
      captureError,
      loading: modelsLoading,
      pending,
      query: modelFilter,
      onQueryChange: setModelFilter,
      activeCategory: addCategory,
      onCategoryChange: (category) => setAddCategory((current) => current === category ? undefined : category),
      selectedModelId: addModelId,
      pinnedModelIds,
      onSelect: (modelId) => {
        setAddModelId(modelId);
        void add(modelId);
      },
      onTogglePinned: (modelId) => { void toggleModelPinned(modelId); },
      onSelectCapture: (capture) => { void addCapture(capture); },
      onClose: closeAddPicker
    };
  }, [add, addCapture, addCategory, addCell, addModelId, addPickerOpen, captures, captureError, captureFolders, closeAddPicker, modelFilter, models, modelsLoading, pending, pinnedModelIds, replaceTarget, toggleModelPinned]);

  const copy = useCallback(async () => {
    if (!topologyComplete || snapshot.gridLayoutComplete === false) {
      notice("The Grid layout is incomplete. Block details are unavailable until the current preset refresh succeeds.");
      return;
    }
    const block = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId && candidate.column >= 0 && candidate.modelId !== undefined);
    if (!block) { notice("Select a Grid block before copying its settings."); return; }
    const sequence = ++copyRequest.current;
    try {
      const next = connected ? await gateway.blockDetails(block.row, block.column, snapshot.presetName, requirePresetIdentity(snapshot)) : demoBlockDetails(block, snapshot.activeScene);
      if (sequence !== copyRequest.current) return;
      assertBlockContext(next, block, snapshot.activeScene);
      setClipboard(next);
      notice(`Copied ${next.name} device parameters. Paste is available on another instance of the same model.`);
    } catch (error) { if (sequence === copyRequest.current) fail(error); }
  }, [connected, fail, gateway, notice, selectedBlockId, snapshot.activeScene, snapshot.blocks, snapshot.gridLayoutComplete, snapshot.presetIdentity, snapshot.presetName, topologyComplete]);

  const paste = useCallback(async () => {
    const source = clipboard;
    const target = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId && candidate.column >= 0 && candidate.modelId !== undefined);
    if (!source || !target) { notice("Copy a device and select a compatible destination before pasting."); return; }
    if (target.modelId !== source.modelId) { notice(`Settings can only be pasted to the same model. ${target.name} does not match ${source.name}.`); return; }
    if (!connected || pending) { notice(!connected ? "Connect the Quad Cortex before pasting live block settings." : "A device command is already in progress."); return; }
    setPending(true);
    notice(`Pasting ${source.name} settings to ${target.name}…`);
    const editorSequence = request.current;
    const editorCurrent = () => editorSequence === request.current
      && currentDetails.current?.row === target.row && currentDetails.current.column === target.column
      && currentDetails.current.modelId === target.modelId && currentDetails.current.scene === snapshot.activeScene;
    try {
      const destination = await gateway.blockDetails(target.row, target.column, snapshot.presetName, requirePresetIdentity(snapshot));
      assertBlockContext(destination, target, snapshot.activeScene);
      const changes = source.parameters.flatMap((parameter) => {
        const current = destination.parameters.find((candidate) => candidate.index === parameter.index);
        return parameter.writable && parameter.normalizedValue !== null && current?.writable && current.normalizedValue !== null && Math.abs(parameter.normalizedValue - current.normalizedValue) >= .000001
          ? [{ index: parameter.index, before: current.normalizedValue, after: parameter.normalizedValue }] : [];
      });
      if (!changes.length) { notice(`${target.name} already has the copied settings.`); return; }
      const applyChanges = async (current: PresetSnapshot, reverse: boolean, observed?: BlockDetails) => {
        // Undo and redo must recheck the target too. Equal parameter indices or
        // values do not make a replacement model compatible with this clipboard.
        const block = observed ?? await gateway.blockDetails(target.row, target.column, current.presetName, requirePresetIdentity(current));
        assertBlockContext(block, target, snapshot.activeScene);
        // Reject a stale compound inverse before changing any of its operands.
        // Native expected-value guards still protect each subsequent write.
        if (changes.some(change => {
          const parameter = block.parameters.find(item => item.index === change.index);
          const expected = reverse ? change.after : change.before;
          return !parameter?.writable || parameter.normalizedValue == null
            || !Number.isFinite(parameter.normalizedValue) || Math.abs(parameter.normalizedValue - expected) > .00001;
        })) throw new Error("A pasted parameter changed before applying the clipboard. Refresh and retry.");
        let verifiedBlock = block;
        let working = current;
        let last: DeviceActionResult | undefined;
        for (const change of changes) {
          const value = reverse ? change.before : change.after;
          const expected = reverse ? change.after : change.before;
          const result = await gateway.setParameter(target.row, target.column, change.index, value, expected, snapshot.activeScene, requireModelId(target), working.presetName, requirePresetIdentity(snapshot));
          assertBlockContext(result.block, target, snapshot.activeScene);
          const confirmed = result.block.parameters.find(parameter => parameter.index === change.index)?.normalizedValue;
          if (confirmed == null || Math.abs(confirmed - value) > .00001) throw new Error("A pasted parameter changed before confirmation. Refresh and retry.");
          verifiedBlock = result.block;
          last = result;
          working = last.snapshot ?? working;
        }
        if (changes.some(change => {
          const value = verifiedBlock.parameters.find(parameter => parameter.index === change.index)?.normalizedValue;
          return value == null || Math.abs(value - (reverse ? change.before : change.after)) > .00001;
        })) throw new Error("A pasted parameter changed while applying the clipboard. Refresh and retry.");
        return { ...(last as DeviceActionResult), snapshot: working, detail: `${reverse ? "Restored" : "Pasted"} ${changes.length} ${target.name} parameter${changes.length === 1 ? "" : "s"}.` };
      };
      const result = await applyChanges(snapshot, false, destination);
      reconcile(result.snapshot ?? snapshot);
      recordHistory({ label: `paste ${source.name} device`, execute: (current) => applyChanges(current, true), redo: (current) => applyChanges(current, false) });
      if (editorCurrent()) {
        const refreshed = await gateway.blockDetails(target.row, target.column, snapshot.presetName, requirePresetIdentity(snapshot));
        if (editorCurrent()) {
          assertBlockContext(refreshed, target, snapshot.activeScene);
          editor.load(refreshed);
        }
      }
      notice(result.detail);
    } catch (error) {
      fail(error);
      try { reconcile(await gateway.currentSnapshot()); } catch { /* Live state will reconcile. */ }
    } finally { setPending(false); }
  }, [clipboard, connected, details?.column, details?.row, editor, fail, gateway, notice, pending, reconcile, recordHistory, selectedBlockId, setPending, snapshot]);

  const saveDevicePreset = useCallback(async (name: string) => {
    const block = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId && candidate.column >= 0 && candidate.modelId !== undefined);
    if (!block) throw new Error("Select a Grid block before saving its parameters.");
    if (!connected || pending) throw new Error(!connected ? "Connect the Quad Cortex before saving a device preset." : "A device command is already in progress.");
    if (!name.trim()) throw new Error("Enter a device preset name.");
    setPending(true);
    try {
      const result = await gateway.createModelPreset(block.row, block.column, requireModelId(block), name.trim(), snapshot.presetName, requirePresetIdentity(snapshot));
      notice(result.detail || `Saved ${name.trim()} as a device preset.`);
    } catch (error) { fail(error); throw error; }
    finally { setPending(false); }
  }, [connected, fail, gateway, notice, pending, selectedBlockId, setPending, snapshot]);

  const loadLooperAssignments = useCallback(async () => {
    if (!deviceConnected) return [3, 4, 5, 6, 0, 1, 7, 2];
    const settings = await gateway.generalSettings();
    const assignments = settings.looperStompAssignments;
    if (!assignments || assignments.length !== 8 || [...assignments].sort((a, b) => a - b).some((value, index) => value !== index)) {
      throw new Error("The Quad Cortex did not return a complete Looper X A–H assignment.");
    }
    return assignments;
  }, [deviceConnected, gateway]);

  const saveLooperAssignments = useCallback(async (assignments: number[]) => {
    if (!deviceConnected || pending) throw new Error(!deviceConnected ? "Connect the Quad Cortex before changing Looper X actions." : "A device command is already in progress.");
    setPending(true);
    try {
      const current = await loadLooperAssignments();
      const result = await gateway.setLooperStompAssignments(assignments, current);
      notice(result.detail || "Looper X footswitch actions updated.");
    } catch (error) { fail(error); throw error; }
    finally { setPending(false); }
  }, [deviceConnected, fail, gateway, loadLooperAssignments, notice, pending, setPending]);

  return {
    details, detailsLoading, moveDestination, setMoveDestination, moveDestinationRow, setMoveDestinationRow, footswitchDraft, setFootswitchDraft,
    footswitchAssignmentPending, setFootswitchAssignmentPending, models, modelsLoading, modelFilter,
    setModelFilter, filteredModels, addCell, setAddCell, addModelId, setAddModelId, clipboard,
    openBlock, openRoutingNode, openLaneControl, close, closeOnDevice, openManagement: onOpenManagement, move, moveBlockTo, assignFootswitch,
    remove, removeBlock, openAdd, openReplace, add, addPickerProps, closeAddPicker, copy, paste,
    saveDevicePreset, loadLooperAssignments, saveLooperAssignments,
    /** Forget the copied device settings: they were read from the transport that was active then. */
    clearClipboard: () => setClipboard(undefined)
  };
}
