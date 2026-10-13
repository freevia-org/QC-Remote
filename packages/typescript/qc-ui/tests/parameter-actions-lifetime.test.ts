import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import type { BlockDetails, BlockParameter, GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import type { ParameterWorkflowOptions } from "../src/use-parameter-workflow.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useParameterWorkflow } = await import("../src/use-parameter-workflow.ts");
const { useQcWorkflows } = await import("../src/use-qc-workflows.ts");
const { IDLE_PLACEHOLDER_SNAPSHOT } = await import("../src/device-placeholder.ts");
const { useGuardedMutation } = await import("../src/use-guarded-mutation.ts");
const { PresetLifetime } = await import("@qc-remote/core");

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((yes) => { resolve = yes; });
  return { promise, resolve };
}
const parameter = { index: 0, name: "Gain", sceneMode: false, normalizedValue: .5, writable: true, options: [] } as unknown as BlockParameter;
const block = (column = 0, sceneMode = false) => ({ row: 0, column, modelId: 42, name: "Amp", scene: 0,
  parameters: [{ ...parameter, sceneMode }] }) as unknown as BlockDetails;

function mount() {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const firstRead = deferred<BlockDetails>();
  const calls: string[] = [], loaded: BlockDetails[] = [], history: unknown[] = [], snapshots: unknown[] = [], failures: unknown[] = [], pending: boolean[] = [];
  let reads = 0;
  const gateway = {
    blockDetails: async () => { calls.push("read"); return ++reads === 1 ? firstRead.promise : block(0, true); },
    resetBlockParameters: async () => { calls.push("reset"); return { detail: "reset", snapshot: { dirty: true } }; },
    setParameterSceneMode: async () => { calls.push("scene"); return { detail: "scene", snapshot: { dirty: true } }; }
  } as unknown as GatewayTransport;
  const snapshot = { presetIdentity: "id", presetName: "preset", setlistKey: "setlist", presetPosition: 0, activeScene: 0 } as PresetSnapshot;
  const lifetime = new PresetLifetime(snapshot);
  let options = {
    gateway, snapshot, connected: true, pending: false, commandSessionKey: gateway,
    controller: { lifetime, cancelInteraction() {} },
    editor: { details: block(), drafts: {}, page: 0, load: (value: BlockDetails) => loaded.push(value) },
    setPending: (value: boolean) => pending.push(value), notice() {}, fail: (error: unknown) => failures.push(error),
    recordHistory: (value: unknown) => history.push(value), reconcile: (value: unknown) => snapshots.push(value)
  } as unknown as ParameterWorkflowOptions;
  let workflow!: ReturnType<typeof useParameterWorkflow>, peer!: ReturnType<typeof useGuardedMutation>;
  function Probe() { workflow = useParameterWorkflow(options); peer = useGuardedMutation(options, "editing"); return null; }
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(Probe)); });
  return { firstRead, calls, loaded, history, snapshots, failures, pending, lifetime,
    get workflow() { return workflow; }, get peer() { return peer; },
    select(details: BlockDetails | undefined) {
      options = { ...options, editor: { ...options.editor, details } };
      renderer.update(React.createElement(Probe));
    }, close() { renderer.unmount(); }
  };
}
const actions = [
  { name: "reset", invoke: (host: ReturnType<typeof mount>) => host.workflow.resetDefaults(), writes: "reset" },
  { name: "scene", invoke: (host: ReturnType<typeof mount>) => host.workflow.toggleSceneMode(parameter), writes: "scene" }
];

test("composed parameter workflow and a host session peer retain one wire lease across pending rerenders", async () => {
  const read = deferred<BlockDetails>();
  const writes: string[] = [], pendingValues: boolean[] = [];
  const gateway = { blockDetails: async () => read.promise, resetBlockParameters: async () => { writes.push("reset"); return { detail: "reset" }; } } as unknown as GatewayTransport;
  const snapshot = { ...IDLE_PLACEHOLDER_SNAPSHOT, presetIdentity: "preset", setlistKey: "A", presetPosition: 0, activeScene: 0 };
  const lifetime = new PresetLifetime(snapshot);
  const controller = { snapshot, snapshotRef: { current: snapshot }, lifetime, cancelInteraction() {}, setSnapshot() {}, acceptSnapshot() {} };
  const editor = { details: block(), drafts: {}, page: 0, load() {}, close() {}, setPage() {}, draft() {}, updateParameter() {}, updateParameters() {}, setScene() {} };
  let workflow!: ReturnType<typeof useQcWorkflows>, peer!: ReturnType<typeof useGuardedMutation>, rerender!: () => void;
  function Probe() {
    const [pending, setPending] = React.useState(false);
    const [, setExtra] = React.useState(0);
    rerender = () => setExtra((value) => value + 1);
    const physicalKey = "device:session-A:ready";
    const setter = React.useCallback((value: React.SetStateAction<boolean>) => { setPending((previous) => {
      const next = typeof value === "function" ? value(previous) : value; pendingValues.push(next); return next;
    }); }, []);
    workflow = useQcWorkflows({ controller, gateway, commandSessionKey: physicalKey, transport: {}, editor, selectedBlockId: "", setSelectedBlockId() {}, connected: true, demo: false,
      pending, setPending: setter, prompts: {}, notice() {}, fail(error: unknown) { throw error; } } as unknown as Parameters<typeof useQcWorkflows>[0]);
    peer = useGuardedMutation({ connected: true, pending, setPending: setter, sessionKey: physicalKey, commandSessionKey: physicalKey, notice() {}, fail(error) { throw error; } }, "host editing");
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(Probe)); });
  try {
    let operation!: Promise<void>;
    await act(async () => { operation = workflow.parameter.resetDefaults(); });
    act(() => rerender());
    let admitted!: boolean;
    await act(async () => { admitted = await peer.run({ write: async () => { writes.push("peer"); } }); });
    assert.equal(admitted, false);
    assert.deepEqual(writes, []);
    assert.ok(pendingValues.every(Boolean), "unrelated rerenders do not clear the in-flight physical lease");
    await act(async () => { read.resolve(block()); await operation; });
    assert.deepEqual(writes, ["reset"]);
    await act(async () => { admitted = await peer.run({ write: async () => { writes.push("peer"); } }); });
    assert.equal(admitted, true);
    assert.deepEqual(writes, ["reset", "peer"]);
  } finally { act(() => renderer.unmount()); }
});

for (const action of actions) {
  for (const boundary of ["switch", "close", "unmount", "same-address reload"] as const) {
    test(`${action.name} refuses its old wire write after a deferred read and ${boundary}`, async () => {
      const host = mount();
      let operation!: Promise<void>;
      await act(async () => { operation = action.invoke(host); });
      assert.deepEqual(host.calls, ["read"]);
      act(() => {
        if (boundary === "unmount") host.close();
        else if (boundary === "same-address reload") host.lifetime.invalidate();
        else host.select(boundary === "close" ? undefined : block(1));
      });
      await act(async () => { host.firstRead.resolve(block()); await operation; });
      assert.deepEqual(host.calls, ["read"]);
      assert.deepEqual(host.loaded, []);
      assert.deepEqual(host.history, []);
      assert.deepEqual(host.snapshots, []);
      assert.deepEqual(host.failures, []);
      assert.deepEqual(host.pending, [true, false], "the exact old lease settles even after editor retirement");
      if (boundary !== "unmount") act(() => host.close());
    });
  }
  test(`${action.name} holds the shared command slot before React rerenders and completes current effects`, async () => {
    const host = mount();
    let operation!: Promise<void>;
    await act(async () => {
      operation = action.invoke(host);
      void action.invoke(host);
      await host.peer.run({ write: async () => { host.calls.push("peer"); } });
    });
    assert.deepEqual(host.calls, ["read"]);
    assert.deepEqual(host.pending, [true]);
    await act(async () => { host.firstRead.resolve(block()); await operation; });
    assert.deepEqual(host.calls, ["read", action.writes, "read"]);
    assert.equal(host.loaded.length, 1);
    assert.equal(host.snapshots.length, 1);
    assert.equal(host.history.length, action.name === "scene" ? 1 : 0);
    assert.deepEqual(host.failures, []);
    assert.deepEqual(host.pending, [true, false]);
    await act(async () => { await host.peer.run({ write: async () => { host.calls.push("peer"); } }); });
    assert.equal(host.calls.at(-1), "peer");
    act(() => host.close());
  });
}

test("fresh disabled parameter readback refuses a linked edit before its first wire write", async () => {
  const writes: number[] = [], failures: unknown[] = [];
  const initial = block();
  let enabled = false;
  const gateway = {
    async blockDetails() { return {...initial,parameters:initial.parameters.map((item) => ({...item,enabled}))}; },
    async setParameter(_row: number,_column: number,index: number,value: number) {
      writes.push(index); return {detail:"written",block:{...initial,parameters:initial.parameters.map((item) => ({...item,enabled,normalizedValue:value}))}};
    }
  } as unknown as GatewayTransport;
  const snapshot = {...IDLE_PLACEHOLDER_SNAPSHOT,presetIdentity:"parameter-enabled",setlistKey:"A",presetPosition:0,activeScene:0};
  const controller = { lifetime:new PresetLifetime(snapshot),updateInteraction(){},releaseInteraction(){},cancelInteraction(){} };
  let workflow!: ReturnType<typeof useParameterWorkflow>;
  function Probe() {
    workflow=useParameterWorkflow({gateway,snapshot,connected:true,pending:false,controller,
      editor:{details:initial,drafts:{},page:0,load(){}},setPending(){},notice(){},fail(error:unknown){failures.push(error);},recordHistory(){},reconcile(){}
    } as unknown as ParameterWorkflowOptions);
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer=create(React.createElement(Probe)); });
  try {
    await act(async () => { await workflow.commitBatch([{parameter:initial.parameters[0],value:.7}]); });
    assert.deepEqual(writes,[]);
    assert.equal(failures.length,1);
    enabled=true;
    await act(async () => { await workflow.commitBatch([{parameter:initial.parameters[0],value:.7}]); });
    assert.deepEqual(writes,[0],"healthy fresh read remains writable");
    assert.equal(failures.length,1);
  } finally { act(() => renderer.unmount()); }
});