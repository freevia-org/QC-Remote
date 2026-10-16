import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useGridWorkflow } = await import("../src/use-grid-workflow.ts");
const { useDeviceHistory } = await import("../src/use-device-history.ts");
const { useQcDeviceUndo } = await import("../src/use-qc-device-undo.ts");

for (const outcome of ["verified", "accepted_unverified", "refused"] as const) {
  test(`block removal ${outcome} uses honest history instead of reconstructing partial parameters`, async () => {
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
    const calls: string[] = [], notices: string[] = [];
    const block = { id: "amp", row: 0, column: 0, modelId: 42, name: "Amp" };
    const snapshot = { ...demoSnapshot, presetIdentity: "id", blocks: [block] };
    const gateway = {
      removeBlock: async () => {
        calls.push("remove");
        if (outcome === "refused") throw new Error("remove refused");
        return { detail: outcome === "verified" ? "Removed and verified." : "Removal sent, not confirmed.", verification: outcome,
          ...(outcome === "verified" ? { snapshot: { ...snapshot, blocks: [] } } : {}) };
      },
      undo: async () => { calls.push("native.undo"); return { detail: "Native history undone.", snapshot }; },
      currentSnapshot: async () => snapshot
    };
    const options = { gateway, snapshot, connected: true, pending: false, setPending() {},
      notice: (message: string) => notices.push(message), fail: (error: Error) => notices.push(error.message), reconcile() {} };
    let history, grid, undo, renderer;
    function Probe() {
      history = useDeviceHistory(options as never);
      grid = useGridWorkflow({ ...options, selectedBlockId: "", setSelectedBlockId() {}, editor: { close() {} },
        recordHistory: history.record, clearHistory: history.clear } as never);
      undo = useQcDeviceUndo({ ...options, history } as never);
      return null;
    }
    await act(async () => { renderer = create(React.createElement(Probe)); });
    await act(async () => { history.record({ label: "old action", execute: async () => { calls.push("old inverse"); return { detail: "old" }; }, redo: async () => ({ detail: "old" }) }); });
    await act(async () => { undo.consume([{ kind: "history", undoAvailable: true }]); });
    await act(async () => { await grid.removeBlock(block); });
    if (outcome === "refused") {
      assert.equal(history.undoEntry?.label, "old action");
      assert.deepEqual(calls, ["remove"]);
      assert.equal(notices.at(-1), "remove refused");
    } else {
      assert.equal(history.undoEntry?.label, "device edit");
      assert.equal(undo.undoLabel, "device edit", "the generic entry delegates to full native history, not a reconstructed inverse");
      await act(async () => { await undo.undo(); });
      assert.deepEqual(calls, ["remove", "native.undo"]);
      if (outcome === "accepted_unverified") assert.ok(notices.includes("Removal sent, not confirmed."));
      assert.equal(notices.some((message) => /restored.*settings/i.test(message)), false);
    }
    await act(async () => renderer.unmount());
  });
}

for (const refused of [false, true]) {
  test(`model replacement ${refused ? "refused" : "accepted"} never offers a model-only inverse as complete Undo`, async (t) => {
    const calls: string[] = [], notices: string[] = [];
    const block = { id: "amp", row: 0, column: 0, modelId: 42, name: "Amp" };
    const snapshot = { ...demoSnapshot, presetIdentity: "id", blocks: [block] };
    const gateway = {
      listModels: async () => ({ models: [{ id: 43, name: "Other Amp" }] }),
      pinnedModels: async () => ({ models: [] }),
      replaceBlock: async () => { calls.push("replace"); if (refused) throw new Error("replacement refused"); return { detail: "Replacement sent, not confirmed.", verification: "accepted_unverified" }; },
      undo: async () => { calls.push("native.undo"); return { detail: "Native history undone.", snapshot }; },
      currentSnapshot: async () => snapshot
    };
    const options = { gateway, snapshot, connected: true, pending: false, setPending() {},
      notice: (message: string) => notices.push(message), fail: (error: Error) => notices.push(error.message), reconcile() {} };
    let history, grid, undo, renderer;
    function Probe() {
      history = useDeviceHistory(options as never);
      grid = useGridWorkflow({ ...options, selectedBlockId: "amp", setSelectedBlockId() {}, editor: { close() {} },
        recordHistory: history.record, clearHistory: history.clear } as never);
      undo = useQcDeviceUndo({ ...options, history } as never);
      return null;
    }
    await act(async () => { renderer = create(React.createElement(Probe)); });
    t.after(async () => { await act(async () => renderer.unmount()); });
    await act(async () => history.record({ label: "old action", execute: async () => { calls.push("old inverse"); return { detail: "old" }; }, redo: async () => ({ detail: "old" }) }));
    await act(async () => undo.consume([{ kind: "history", undoAvailable: true }]));
    await act(async () => { await grid.openReplace(); });
    await act(async () => { await grid.add(43); });
    if (refused) {
      assert.equal(history.undoEntry?.label, "old action");
      assert.deepEqual(calls, ["replace"]);
    } else {
      assert.equal(history.undoEntry?.label, "device edit");
      assert.equal(undo.undoLabel, "device edit");
      await act(async () => { await undo.undo(); });
      assert.deepEqual(calls, ["replace", "native.undo"]);
      assert.ok(notices.includes("Replacement sent, not confirmed."));
    }
  });
}
