import { QC_GRID_COLUMNS, requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useMemo, useState, type Dispatch, type SetStateAction } from "react";
import type { GatewayTransport } from "@qc-remote/client";
import { inputRouteOptions, outputRouteOptions, routeDraft, routeDraftsFromSnapshot, routeOptionValue, routeOptionsForRow, updateRouteDraft, type RouteDrafts } from "@qc-remote/core";
import type { CorOsRoutingPickerState } from "./quad-cortex-surface";
import type { DeviceMutationWorkflowOptions } from "./workflow-options";

export type RoutePicker = { row: number; side: "input" | "output" | "split" | "mix" };

export type RoutingWorkflowOptions = DeviceMutationWorkflowOptions;

export interface RoutingWorkflowController {
  drafts: RouteDrafts;
  setDrafts: Dispatch<SetStateAction<RouteDrafts>>;
  picker?: RoutePicker;
  pickerProps?: CorOsRoutingPickerState;
  openPicker(row: number, side: "input" | "output"): void;
  closePicker(): void;
  applyRoute(row: number, side: "input" | "output", selected?: number): Promise<void>;
  applySplit(row: number): Promise<void>;
  /**
   * The direct replacement for the old dialog's Branch <select> pair, reached
   * from a real tap target on an even, unsplit row (quad-cortex-surface.tsx's
   * rowActionStops) rather than a menu. Opens the same coros-route-picker
   * surface already used for input/output selection, first for the split
   * column and then, on selection, for the rejoin column - each choice reuses
   * the existing verified applySplit/setChainSplit path, so no new write
   * path is introduced for this flow.
   */
  openSplitPlacement(row: number): void;
}

/** Shared route validation, mutation, verification, and history controller. */
export function useRoutingWorkflow(options: RoutingWorkflowOptions): RoutingWorkflowController {
  const { gateway, snapshot, connected, pending, setPending, reconcile, recordHistory, notice, fail } = options;
  const [drafts, setDrafts] = useState<RouteDrafts>({});
  const [picker, setPicker] = useState<RoutePicker>();

  const openPicker = useCallback((row: number, side: "input" | "output") => {
    const route = snapshot.routes.find((candidate) => candidate.row === row);
    if (!connected || route?.inputId === undefined || route.outputId === undefined) {
      notice(!connected ? "Connect the Quad Cortex before changing signal routing." : "Refresh the complete device state before changing this route.");
      return;
    }
    setPicker((current) => current?.row === row && current.side === side ? undefined : { row, side });
    notice(`Select row ${row + 1} ${side} on the Quad Cortex screen.`);
  }, [connected, notice, snapshot.routes]);

  const applyRoute = useCallback(async (row: number, side: "input" | "output", selected?: number) => {
    const route = snapshot.routes.find((candidate) => candidate.row === row);
    const draft = drafts[row];
    const expected = side === "input"
      ? routeOptionValue("input", route?.inputId, route?.input ?? "Internal")
      : routeOptionValue("output", route?.outputId, route?.output ?? "Internal");
    const desired = selected ?? (side === "input" ? draft?.inputId : draft?.outputId);
    if (desired === undefined || pending) return;
    if (!connected) { notice("Connect the Quad Cortex before changing signal routing."); return; }
    if (expected === desired) { setPicker(undefined); return; }
    setPending(true);
    notice(`Updating row ${row + 1} ${side}…`);
    try {
      const execute = (value: number, previous: number, presetName: string) => side === "input"
        ? gateway.setChainInput(row, value, previous, presetName, requirePresetIdentity(snapshot))
        : gateway.setChainOutput(row, value, previous, presetName, requirePresetIdentity(snapshot));
      const result = await execute(desired, expected, snapshot.presetName);
      if (result.snapshot) {
        reconcile(result.snapshot);
        setDrafts(routeDraftsFromSnapshot(result.snapshot));
      }
      recordHistory({
        label: `row ${row + 1} ${side}`,
        execute: (current) => execute(expected, desired, current.presetName),
        redo: (current) => execute(desired, expected, current.presetName)
      });
      setPicker(undefined);
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [connected, drafts, fail, gateway, notice, pending, reconcile, recordHistory, setPending, snapshot.presetIdentity, snapshot.presetName, snapshot.routes]);

  /**
   * The write both applySplit (reading a fully-drafted row) and the split
   * placement flow (which never populates the whole-row drafts, only the one
   * row it is placing) share, so there is exactly one setChainSplit call site
   * for the whole feature.
   */
  const commitSplit = useCallback(async (row: number, splitColumn: number | null, mixColumn: number | null) => {
    const route = snapshot.routes.find((candidate) => candidate.row === row);
    if (!route || pending) return;
    if (!connected) { notice("Connect the Quad Cortex before changing signal routing."); return; }
    const expectedSplit = route.splitColumn ?? null;
    const expectedMix = route.splitColumn === undefined ? null : route.mixColumn ?? -1;
    if (splitColumn === expectedSplit && mixColumn === expectedMix) return;
    const description = splitColumn === null ? "return it to a serial path" : `branch at column ${splitColumn + 1}${mixColumn === -1 ? " without a rejoin" : ` and rejoin at column ${Number(mixColumn) + 1}`}`;
    setPending(true);
    notice(`Updating row ${row + 1} parallel routing…`);
    try {
      const result = await gateway.setChainSplit(row, splitColumn, mixColumn, expectedSplit, expectedMix, snapshot.presetName, requirePresetIdentity(snapshot));
      if (result.snapshot) {
        reconcile(result.snapshot);
        setDrafts(routeDraftsFromSnapshot(result.snapshot));
      }
      recordHistory({
        label: `row ${row + 1} branch routing`,
        execute: (current) => gateway.setChainSplit(row, expectedSplit, expectedMix, splitColumn, mixColumn, current.presetName, requirePresetIdentity(current)),
        redo: (current) => gateway.setChainSplit(row, splitColumn, mixColumn, expectedSplit, expectedMix, current.presetName, requirePresetIdentity(current))
      });
      notice(result.detail);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [connected, fail, gateway, notice, pending, reconcile, recordHistory, setPending, snapshot.presetIdentity, snapshot.presetName, snapshot.routes]);

  const applySplit = useCallback(async (row: number) => {
    const draft = drafts[row];
    if (!draft) return;
    await commitSplit(row, draft.splitColumn, draft.mixColumn);
  }, [commitSplit, drafts]);

  const openSplitPlacement = useCallback((row: number) => {
    if (!connected) { notice("Connect the Quad Cortex before adding a signal split."); return; }
    const route = snapshot.routes.find((candidate) => candidate.row === row);
    if (!route) { notice("Refresh the complete device state before adding a split."); return; }
    if (route.splitColumn !== undefined) { notice(`Row ${row + 1} already branches - open its Splitter marker on the Grid to edit it.`); return; }
    setDrafts((current) => updateRouteDraft(current, route, { splitColumn: null, mixColumn: null }));
    setPicker({ row, side: "split" });
  }, [connected, notice, snapshot.routes]);

  const pickerProps = useMemo<CorOsRoutingPickerState | undefined>(() => {
    if (!picker) return undefined;
    if (picker.side === "split" || picker.side === "mix") {
      const route = snapshot.routes.find((candidate) => candidate.row === picker.row);
      const draft = drafts[picker.row] ?? (route ? routeDraft(route) : { inputId: 0, outputId: 0, splitColumn: null as number | null, mixColumn: null as number | null });
      const isSplit = picker.side === "split";
      const options = isSplit
        ? Array.from({ length: QC_GRID_COLUMNS }, (_, column) => [column, `Split at column ${column + 1}`] as const)
        : [[-1, "No rejoin"] as const, ...Array.from({ length: QC_GRID_COLUMNS }, (_, column) => column)
            .filter((column) => draft.splitColumn !== null && column > draft.splitColumn)
            .map((column) => [column, `Rejoin at column ${column + 1}`] as const)];
      const value = isSplit ? (draft.splitColumn ?? 0) : (draft.mixColumn ?? -1);
      return {
        row: picker.row,
        side: picker.side,
        options,
        value,
        disabled: pending,
        onSelect: (selected) => {
          if (isSplit) {
            setDrafts((current) => ({ ...current, [picker.row]: { ...draft, splitColumn: selected, mixColumn: -1 } }));
            setPicker({ row: picker.row, side: "mix" });
          } else {
            setPicker(undefined);
            void commitSplit(picker.row, draft.splitColumn, selected);
          }
        },
        onClose: () => setPicker(undefined)
      };
    }
    const side: "input" | "output" = picker.side === "input" ? "input" : "output";
    const route = snapshot.routes.find((candidate) => candidate.row === picker.row);
    const value = side === "input"
      ? routeOptionValue("input", route?.inputId, route?.input ?? "Internal")
      : routeOptionValue("output", route?.outputId, route?.output ?? "Internal");
    return {
      row: picker.row,
      side,
      options: routeOptionsForRow(side, picker.row, value, snapshot.routes),
      value,
      disabled: pending,
      onSelect: (selected) => void applyRoute(picker.row, side, selected),
      onClose: () => setPicker(undefined)
    };
  }, [applyRoute, commitSplit, drafts, pending, picker, snapshot.routes]);

  return { drafts, setDrafts, picker, pickerProps, openPicker, closePicker: () => setPicker(undefined), applyRoute, applySplit, openSplitPlacement };
}
