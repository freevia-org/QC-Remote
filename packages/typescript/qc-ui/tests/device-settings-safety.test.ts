import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import type { GatewayTransport } from "@qc-remote/client";
import { registerGatewayRoute } from "../src/gateway-session.ts";
import { openOnUnit, closeOnUnit } from "../src/qc-unit-screens.ts";
import { readQcScreenFollowSetting, writeQcScreenFollowSetting } from "../src/use-adaptive-screen-polling.ts";
import { PresetLifetime } from "@qc-remote/core";
import { demoSnapshot } from "../../qc-core/src/device-reference/demo-snapshot.ts";
import { qcLooperScreenActions } from "../src/qc-device-screen.ts";
register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useDeviceSettingsWorkflow } = await import("../src/use-device-settings-workflow.ts");
const { useQcDeviceViews } = await import("../src/use-qc-device-views.ts");
const { useQcScreenDestination } = await import("../src/use-qc-screen-destination.ts");
const { useQcController } = await import("../src/use-qc-controller.ts");
const { useTunerWorkflow } = await import("../src/use-tuner-workflow.ts");
const { useLooperXWorkflow } = await import("../src/use-looper-x-workflow.ts");
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
function mount(gateway: GatewayTransport, lifetime?: PresetLifetime) {
  const errors: unknown[] = [];
  const seen: { current?: ReturnType<typeof useDeviceSettingsWorkflow> } = {};
  function Probe() {
    const [pending, setPending] = React.useState(false);
    seen.current = useDeviceSettingsWorkflow({ gateway, lifetime, connected: true, pending, setPending, notice() {}, fail(error) { errors.push(error); } });
    return null;
  }
  let root!: ReactTestRenderer;
  act(() => { root = create(React.createElement(Probe)); });
  return { errors, get value() { return seen.current!; }, root };
}
const family = { row1: true, row2: false, row3: true, row4: true };
function mountControls(gateway:GatewayTransport,lifetime?:PresetLifetime) {
  const errors:unknown[]=[];let controls!:{tuner:ReturnType<typeof useTunerWorkflow>;looper:ReturnType<typeof useLooperXWorkflow>;pending:boolean};let root!:ReactTestRenderer;
  function Probe(){
    const[fallbackPending,setFallbackPending]=React.useState(false);
    // Both hosts derive physical-session pending from the actual admission
    // store. An obsolete captured boolean must not outlive a session reset.
    const sharedPending=React.useSyncExternalStore(
      React.useCallback((listener:()=>void)=>lifetime?.mutations.subscribe(listener)??(()=>{}),[lifetime]),
      ()=>lifetime?.mutations.busy??false
    );
    const pending=lifetime?sharedPending:fallbackPending;
    const setPending=(next:boolean)=>{if(!lifetime)setFallbackPending(next);};
    const options={gateway,lifetime,connected:true,pending,setPending,notice(){},fail(error:unknown){errors.push(error);}};
    controls={tuner:useTunerWorkflow(options),looper:useLooperXWorkflow(options),pending};return null;
  }
  act(()=>{root=create(React.createElement(Probe));});return{root,errors,replace(next:GatewayTransport){gateway=next;root.update(React.createElement(Probe));},get value(){return controls;}};
}

test("retained tuner writes cannot recapture a replacement physical session on the same gateway", async (t) => {
  const cases = [
    { name: "input", invoke: (tuner: ReturnType<typeof useTunerWorkflow>) => tuner.setInput(2) },
    { name: "reference", invoke: (tuner: ReturnType<typeof useTunerWorkflow>) => tuner.setReferenceOffset(3) },
    { name: "meter", invoke: (tuner: ReturnType<typeof useTunerWorkflow>) => tuner.setMeterEnabled(true) }
  ];
  assert.equal(cases.length, 3);
  for (const item of cases) await t.test(item.name, async (t) => {
    const sent: unknown[][] = [];
    const lifetime = new PresetLifetime({ setlistKey: "set", presetPosition: 0 });
    const gateway = {
      async tunerSettings() { return { inputPortId: 1, referenceHz: 440, referenceOffsetHz: 0, muted: false, meterEnabled: false }; },
      async setTunerInput(...args: unknown[]) { sent.push(["input", ...args]); return { detail: "input changed" }; },
      async setTunerReference(...args: unknown[]) { sent.push(["reference", ...args]); return { detail: "reference changed" }; },
      async setTunerMeter(...args: unknown[]) { sent.push(["meter", ...args]); return { detail: "meter changed" }; }
    } as unknown as GatewayTransport;
    const host = mountControls(gateway, lifetime);
    t.after(async () => { await act(async () => host.root.unmount()); });
    const oldTuner = host.value.tuner;
    await act(async () => lifetime.reset());
    await act(async () => { item.invoke(oldTuner); });
    assert.deepEqual(sent, [], "the old session's callback must not send a write to the replacement");
    assert.equal(host.errors.length, 0);
    assert.equal(lifetime.mutations.busy, false);
    await act(async () => { item.invoke(host.value.tuner); });
    assert.equal(sent.length, 1, "fresh controls in the new physical session still work");
    assert.equal(host.errors.length, 0);
    assert.equal(lifetime.mutations.busy, false);
  });
});

test("real controller session reset retires old tuner admission without clearing the replacement writer", async (t) => {
  const first = deferred<unknown>(); const next = deferred<unknown>(); const sent: number[] = [];
  const errors: unknown[] = [];
  const gateway = {
    async tunerSettings() { return { referenceHz: 443, referenceOffsetHz: 3 }; },
    setTunerReference(value: number) { sent.push(value); return value === 1 ? first.promise : next.promise; }
  } as unknown as GatewayTransport;
  let controller!: ReturnType<typeof useQcController>;
  let tuner!: ReturnType<typeof useTunerWorkflow>;
  let pending = false;
  function Probe() {
    controller = useQcController(demoSnapshot);
    pending = React.useSyncExternalStore(controller.lifetime.mutations.subscribe, controller.lifetime.mutations.snapshot);
    tuner = useTunerWorkflow({ gateway, lifetime: controller.lifetime, connected: true, pending,
      setPending() {}, notice() {}, fail(error) { errors.push(error); } });
    return null;
  }
  let root!: ReactTestRenderer;
  await act(async () => { root = create(React.createElement(Probe)); });
  t.after(async () => {
    await act(async () => { first.resolve({ detail: "cleanup" }); next.resolve({ detail: "cleanup" }); });
    await act(async () => root.unmount());
  });
  await act(async () => { tuner.setReferenceOffset(1); tuner.setReferenceOffset(2); });
  assert.deepEqual(sent, [1]); assert.equal(pending, true);
  await act(async () => controller.resetCommands());
  assert.equal(pending, false);
  await act(async () => tuner.setReferenceOffset(3));
  assert.deepEqual(sent, [1, 3]); assert.equal(pending, true);
  await act(async () => first.resolve({ detail: "obsolete" }));
  assert.equal(pending, true, "obsolete native completion cannot retire replacement admission");
  await act(async () => next.resolve({ detail: "current" }));
  assert.equal(pending, false); assert.deepEqual(errors, []);
});

test("retained old looper and tuner refresh callbacks cannot acquire replacement loading",async(t)=>{
  let reads=0;const first={async looperStatus(){reads++;return{};},async tunerSettings(){reads++;return{};}}as unknown as GatewayTransport;
  const host=mountControls(first);t.after(async()=>{await act(async()=>host.root.unmount());});
  const oldLooper=host.value.looper.refresh;const oldTuner=host.value.tuner.refresh;
  const oldOpen=host.value.tuner.openTuner;const oldReference=host.value.tuner.setReferenceOffset;
  await act(async()=>host.replace({}as GatewayTransport));
  await act(async()=>{await oldLooper();await oldTuner();oldOpen();oldReference(1);});
  assert.equal(reads,0);assert.equal(host.value.looper.loading,false);assert.equal(host.value.tuner.loading,false);assert.equal(host.errors.length,0);
  assert.equal(host.value.tuner.open,false);assert.equal(host.value.pending,false);
});
test("closed tuner ignores late settings reads",async(t)=>{
  const read=deferred<unknown>();const host=mountControls({tunerSettings:()=>read.promise}as unknown as GatewayTransport);
  t.after(async()=>{await act(async()=>host.root.unmount());});
  await act(async()=>host.value.tuner.openTuner());await act(async()=>host.value.tuner.close());
  await act(async()=>read.resolve({referenceHz:450,referenceOffsetHz:10}));
  assert.equal(host.value.tuner.settings,undefined);assert.equal(host.value.tuner.loading,false);
});
test("cancelling tuner dial drops queued values but keeps original admission until it settles",async(t)=>{
  const first=deferred<unknown>();const next=deferred<unknown>();const sent:number[]=[];
  const host=mountControls({async tunerSettings(){return{referenceHz:443,referenceOffsetHz:3};},setTunerReference(value:number){sent.push(value);return value===1?first.promise:next.promise;}}as unknown as GatewayTransport);
  t.after(async()=>{await act(async()=>host.root.unmount());});
  await act(async()=>host.value.tuner.setReferenceOffset(1));await act(async()=>host.value.tuner.setReferenceOffset(2));
  await act(async()=>host.value.tuner.close());await act(async()=>host.value.tuner.setReferenceOffset(3));
  assert.deepEqual(sent,[1]);assert.equal(host.value.pending,true);
  await act(async()=>first.resolve({detail:"old"}));assert.equal(host.value.pending,false);
  await act(async()=>host.value.tuner.setReferenceOffset(3));
  assert.deepEqual(sent,[1,3]);assert.equal(host.value.pending,true);
  await act(async()=>next.resolve({detail:"new"}));assert.equal(host.value.pending,false);
});
test("looper completion after route switch cannot refresh another device",async(t)=>{
  const write=deferred<unknown>();let reads=0;
  const first={controlLooper:()=>write.promise,async looperStatus(){reads++;return{};}}as unknown as GatewayTransport;
  const second={async controlLooper(){throw new Error("wrong target");},async looperStatus(){reads++;return{};}}as unknown as GatewayTransport;
  const routing=route(first,second);const host=mountControls(routing.routed);
  t.after(async()=>{await act(async()=>host.root.unmount());});let action!:Promise<boolean>;
  await act(async()=>{action=host.value.looper.run("record");});routing.switch();
  await act(async()=>{write.resolve({detail:"old"});await action;});assert.equal(reads,0);assert.equal(host.errors.length,0);
});

test("same-route session reset cancels queued tuner writes and invalidates looper reads",async(t)=>{
  const write=deferred<unknown>();const read=deferred<unknown>();const sent:number[]=[];
  const lifetime=new PresetLifetime({setlistKey:"set",presetPosition:0});
  const host=mountControls({setTunerReference(value:number){sent.push(value);return write.promise;},looperStatus:()=>read.promise}as unknown as GatewayTransport,lifetime);
  t.after(async()=>{await act(async()=>host.root.unmount());});let refresh!:Promise<void>;
  await act(async()=>{host.value.tuner.setReferenceOffset(1);host.value.tuner.setReferenceOffset(2);refresh=host.value.looper.refresh();});
  await act(async()=>lifetime.reset());
  await act(async()=>{write.resolve({detail:"old"});read.resolve({state:"old"});await refresh;});
  assert.deepEqual(sent,[1]);assert.equal(host.value.looper.status,undefined);assert.equal(host.value.looper.loading,false);
  assert.equal(host.value.pending,false);assert.equal(host.errors.length,0);
});

test("looper command readback supersedes a pending refresh and owns its loading completion",async(t)=>{
  const first=deferred<unknown>();let reads=0;
  const host=mountControls({looperStatus(){return ++reads===1?first.promise:Promise.resolve({state:"new"});},async controlLooper(){return{detail:"sent"};}}as unknown as GatewayTransport);
  t.after(async()=>{await act(async()=>host.root.unmount());});let refresh!:Promise<void>;
  await act(async()=>{refresh=host.value.looper.refresh();});assert.equal(host.value.looper.loading,true);
  await act(async()=>host.value.looper.run("record"));assert.equal(host.value.looper.loading,false);
  await act(async()=>{first.resolve({state:"old"});await refresh;});assert.deepEqual(host.value.looper.status,{state:"new"});
});

test("Looper hook reports failure and outer editor never continues on a replacement preset", async(t)=>{
  const lifetime = new PresetLifetime({setlistKey:"set",presetPosition:0});
  let failWrite = true;
  const host = mountControls({ async controlLooper(){ if(failWrite) throw new Error("write refused"); return {detail:"sent"}; }, async looperStatus(){return{};} } as unknown as GatewayTransport,lifetime);
  t.after(async()=>{await act(async()=>host.root.unmount());});
  let accepted: boolean | undefined;
  await act(async()=>{accepted = await host.value.looper.run("close");});
  assert.equal(accepted,false);
  failWrite = false;
  await act(async()=>{accepted = await host.value.looper.run("close");});
  assert.equal(accepted,true);
  const log:string[]=[];
  const workflow = host.value.looper;
  const actions = qcLooperScreenActions({snapshot:{blocks:[{row:0,column:0,modelId:27000,name:"Looper X"} as never]},
    looper:{...workflow,async run(command,value){const result=await workflow.run(command,value); lifetime.observe({setlistKey:"set",presetPosition:1}); return result;}},
    grid:{async openBlock(){log.push("block");}},setView(){log.push("view");},setParameterMenuInitiallyOpen(){log.push("menu");},notice(){}});
  await act(async()=>actions.onMenu());
  assert.deepEqual(log,[]);
  const renderedPresetCurrent = host.value.looper.captureContinuation();
  let sent = 0;
  const retained = qcLooperScreenActions({snapshot:{blocks:[{row:0,column:0,modelId:27000,name:"Looper X"} as never]},
    looper:{...host.value.looper,async run(){sent++;return true;}},current:renderedPresetCurrent,
    grid:{async openBlock(){log.push("block");}},setView(){log.push("view");},setParameterMenuInitiallyOpen(){log.push("menu");},notice(){}});
  await act(async()=>lifetime.observe({setlistKey:"set",presetPosition:2}));
  await act(async()=>retained.onParameters());
  assert.equal(sent,0,"retained render handler cannot dispatch old block on newer preset");
  assert.deepEqual(log,[]);
});
test("unavailable screen-follow storage uses the default and does not break navigation settings", () => {
  const storage = { getItem(): string | null { throw new Error("Storage denied"); }, setItem() { throw new Error("Storage full"); } };
  assert.equal(readQcScreenFollowSetting(storage), true);
  assert.doesNotThrow(() => writeQcScreenFollowSetting(storage, false));
});
test("one master-volume checkbox preserves freshly observed other outputs", async (t) => {
  const calls: unknown[][] = [];
  const host = mount({ async generalSettings() { return { masterVolumeAssignment: { out12: false, out34: true, send12: true, headphones: true } }; }, async setMasterVolumeAssignment(...args: unknown[]) { calls.push(args); return { detail: "Updated" }; } } as unknown as GatewayTransport);
  t.after(async () => { await act(async () => host.root.unmount()); });
  await act(async () => host.value.setMasterVolumeOutput("out12", true));
  assert.deepEqual(calls, [[true, true, true, true]]);
});
test("one bypass row uses fresh complete family state and never writes the other family", async (t) => {
  const calls: unknown[][] = [];
  const gateway = { async generalSettings() { return { globalBypassCab: family }; }, async setGlobalBypass(...args: unknown[]) { calls.push(args); return { detail: "Updated" }; } } as unknown as GatewayTransport;
  const host = mount(gateway); t.after(async () => { await act(async () => host.root.unmount()); });
  await act(async () => host.value.setGlobalBypass("cab", 2, true));
  assert.deepEqual(calls, [[[true, true, true, true], null]]);
  assert.equal(host.errors.length, 0);
});
test("incomplete bypass read refuses before a write instead of inventing disabled rows", async (t) => {
  let writes = 0;
  const gateway = { async generalSettings() { return { globalBypassIr: { row1: true } }; }, async setGlobalBypass() { writes++; } } as unknown as GatewayTransport;
  const host = mount(gateway); t.after(async () => { await act(async () => host.root.unmount()); });
  await act(async () => host.value.setGlobalBypass("ir", 2, true));
  assert.equal(writes, 0); assert.match(String(host.errors[0]), /state is unavailable/);
});
test("retained settings opening cannot acquire or invalidate replacement session loading", async (t) => {
  const current = deferred<unknown>();
  let reads = 0; let identities = 0;
  const lifetime = new PresetLifetime({ setlistKey: "set", presetPosition: 0 });
  const host = mount({
    generalSettings() { reads++; return current.promise; },
    async identity() { identities++; return {}; }
  } as unknown as GatewayTransport, lifetime);
  t.after(async () => { await act(async () => host.root.unmount()); });
  const obsoleteOpen = host.value.openSettings;
  await act(async () => lifetime.reset());
  await act(async () => obsoleteOpen());
  assert.equal(host.value.open, false, "obsolete controls cannot reopen the replacement session's settings");
  assert.equal(host.value.loading, false, "obsolete controls cannot acquire loading without an admitted read");
  assert.equal(reads, 0); assert.equal(identities, 0);
  await act(async () => host.value.openSettings());
  assert.equal(host.value.loading, true); assert.equal(reads, 1); assert.equal(identities, 1);
  await act(async () => obsoleteOpen());
  await act(async () => current.resolve({ screenBrightness: 17 }));
  assert.equal(host.value.settings?.screenBrightness, 17, "obsolete opening cannot invalidate the fresh read");
  assert.equal(host.value.loading, false); assert.equal(host.value.open, true);
  assert.equal(reads, 1); assert.equal(identities, 1); assert.equal(host.errors.length, 0);
});

test("retained settings opening cannot suppress an admitted replacement opening", async (t) => {
  const current = deferred<unknown>(); let reads = 0; let identities = 0;
  const lifetime = new PresetLifetime({ setlistKey: "set", presetPosition: 0 });
  const host = mount({
    generalSettings() { reads++; return current.promise; },
    async identity() { identities++; return {}; }
  } as unknown as GatewayTransport, lifetime);
  t.after(async () => { await act(async () => host.root.unmount()); });
  const obsoleteOpen = host.value.openSettings;
  await act(async () => lifetime.reset());
  await act(async () => host.value.openSettings());
  assert.equal(reads, 1); assert.equal(identities, 1); assert.equal(host.value.loading, true);
  await act(async () => obsoleteOpen());
  await act(async () => current.resolve({ screenBrightness: 23 }));
  assert.equal(host.value.settings?.screenBrightness, 23);
  assert.equal(host.value.loading, false, "an obsolete opening cannot strand the admitted opening's loading lease");
  assert.equal(host.value.open, true);
  assert.equal(reads, 1); assert.equal(identities, 1); assert.equal(host.errors.length, 0);
});

test("retained settings refresh cannot invalidate a replacement session's current read", async (t) => {
  const current = deferred<unknown>(); let reads = 0;
  const lifetime = new PresetLifetime({ setlistKey: "set", presetPosition: 0 });
  const host = mount({ generalSettings() { reads++; return current.promise; } } as unknown as GatewayTransport, lifetime);
  t.after(async () => { await act(async () => host.root.unmount()); });
  const obsoleteRefresh = host.value.refresh;
  await act(async () => lifetime.reset());
  let refresh!: Promise<void>;
  await act(async () => { refresh = host.value.refresh(); });
  assert.equal(reads, 1);
  await act(async () => obsoleteRefresh());
  await act(async () => { current.resolve({ screenBrightness: 19 }); await refresh; });
  assert.equal(host.value.settings?.screenBrightness, 19, "obsolete refresh cannot advance the new read's generation");
  assert.equal(reads, 1); assert.equal(host.errors.length, 0);
});

test("an older settings opening cannot overwrite a newer opening or its loading state", async (t) => {
  const reads = [deferred<unknown>(), deferred<unknown>()]; let count = 0;
  const host = mount({ generalSettings: () => reads[count++].promise, async identity() { return {}; } } as unknown as GatewayTransport);
  t.after(async () => { await act(async () => host.root.unmount()); });
  await act(async () => host.value.openSettings()); await act(async () => host.value.openSettings());
  await act(async () => reads[0].resolve({ screenBrightness: 1 }));
  assert.equal(host.value.settings, undefined); assert.equal(host.value.loading, true);
  await act(async () => reads[1].resolve({ screenBrightness: 8 }));
  assert.equal(host.value.settings?.screenBrightness, 8); assert.equal(host.value.loading, false);
});
test("closing settings invalidates pending reads without showing a late error", async (t) => {
  const read = deferred<unknown>();
  const host = mount({ generalSettings: () => read.promise, identity: () => read.promise } as unknown as GatewayTransport);
  t.after(async () => { await act(async () => host.root.unmount()); });
  await act(async () => host.value.openSettings()); await act(async () => host.value.close());
  await act(async () => read.resolve({ screenBrightness: 7 }));
  assert.equal(host.value.settings, undefined); assert.equal(host.value.identity, undefined);
  assert.equal(host.value.loading, false); assert.equal(host.errors.length, 0);
});

test("unmounted device settings cannot read back a delayed mutation",async()=>{
  const write=deferred<unknown>();let reads=0;
  const host=mount({setGeneralInteger:()=>write.promise,async generalSettings(){reads++;return{};}}as unknown as GatewayTransport);
  let mutation!:Promise<void>;await act(async()=>{mutation=host.value.setScreenBrightness(5);});
  await act(async()=>host.root.unmount());await act(async()=>{write.resolve({detail:"old"});await mutation;});
  assert.equal(reads,0);assert.equal(host.errors.length,0);
});
function route(first: GatewayTransport, second: GatewayTransport) {
  let active = first; let revision = 0;
  const routed = new Proxy({} as GatewayTransport, { get(_target, method) { return (...args: unknown[]) => (active as unknown as Record<string, (...args: unknown[]) => unknown>)[String(method)](...args); } });
  registerGatewayRoute(routed, { active: () => active, revision: () => revision });
  return { routed, switch() { active = second; revision++; } };
}
test("device switch during bypass read cannot redirect its subsequent write", async (t) => {
  const read = deferred<unknown>(); let writes = 0;
  const first = { generalSettings: () => read.promise, async setGlobalBypass() { writes++; } } as unknown as GatewayTransport;
  const second = { async generalSettings() { return { globalBypassCab: family }; }, async setGlobalBypass() { writes++; } } as unknown as GatewayTransport;
  const routing = route(first, second); const host = mount(routing.routed);
  t.after(async () => { await act(async () => host.root.unmount()); });
  let mutation!: Promise<void>;
  await act(async () => { mutation = host.value.setGlobalBypass("cab", 2, true); });
  routing.switch(); await act(async () => { read.resolve({ globalBypassCab: family }); await mutation; });
  assert.equal(writes, 0);
});
test("same-route session reset cancels settings writes but preset changes do not", async (t) => {
  const read = deferred<unknown>(); let writes = 0;
  const lifetime = new PresetLifetime({ setlistKey: "set", presetPosition: 0 });
  const gateway = { generalSettings: () => read.promise, async setGlobalBypass() { writes++; return {detail:"Updated"}; } } as unknown as GatewayTransport;
  const host = mount(gateway, lifetime); t.after(async () => { await act(async () => host.root.unmount()); });
  let mutation!: Promise<void>;
  await act(async () => { mutation = host.value.setGlobalBypass("cab", 2, true); });
  await act(async () => lifetime.reset());
  await act(async () => { read.resolve({ globalBypassCab: family }); await mutation; });
  assert.equal(writes, 0); assert.equal(host.errors.length, 0);
  const newHost = mount(gateway, lifetime); t.after(async () => { await act(async () => newHost.root.unmount()); });
  await act(async () => lifetime.observe({ setlistKey: "set", presetPosition: 1 }));
  await act(async () => newHost.value.setGlobalBypass("cab", 2, true));
  assert.equal(writes, 1);
});
for (const closing of [false, true]) {
  test(`screen ${closing ? "close" : "open"} cannot gesture after a route switch during its screen read`, async () => {
    const read = deferred<{ tree: string }>(); let gestures = 0;
    const first = { graphicsTree: () => read.promise, async tapScreen() { gestures++; } } as unknown as GatewayTransport;
    const second = { async graphicsTree() { return { tree: "zenUI::Grid" }; }, async tapScreen() { gestures++; } } as unknown as GatewayTransport;
    const routing = route(first, second);
    const result = closing ? closeOnUnit(routing.routed, { intervalMs: 1, timeoutMs: 1 }, "settings") : openOnUnit(routing.routed, { intervalMs: 1, timeoutMs: 1 }, "preset-menu");
    routing.switch(); read.resolve({ tree: closing ? "zenUI::SettingsDialog" : "zenUI::Grid" });
    await assert.rejects(result); assert.equal(gestures, 0);
  });
}
test("mirrored screen gesture cannot redirect after its update-safety read", async (t) => {
  const read = deferred<{ tree: string }>(); let gestures = 0;
  const first = { async identity() { return {}; }, graphicsTree: () => read.promise, async tapScreen() { gestures++; } } as unknown as GatewayTransport;
  const second = { async identity() { return {}; }, async graphicsTree() { return { tree: "zenUI::Grid" }; }, async tapScreen() { gestures++; } } as unknown as GatewayTransport;
  const routing = route(first, second);
  let views!: ReturnType<typeof useQcDeviceViews>; let root!: ReactTestRenderer;
  let destination!: ReturnType<typeof useQcScreenDestination>;
  function Probe() { destination = useQcScreenDestination(); views = useQcDeviceViews({ gateway: routing.routed, snapshot: { deviceName: "QC", setlistKey: "set" }, session: { demo: false, hardwareReady: true, sessionReady: false }, destination, workflows: {} as never, notice() {}, fail() { throw new Error("obsolete gesture must stay silent"); } }); return null; }
  await act(async () => { root = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => root.unmount()); });
  await act(async () => { destination.setView("unit-mirror"); });
  const gesture = views.forwardGesture(20, 20, 20, 20, "tap");
  routing.switch(); read.resolve({ tree: "zenUI::Grid" }); await gesture;
  assert.equal(gestures, 0);
});
for (const kind of ["close-io", "info", "open-io"] as const) {
  test(`stale ${kind} completion cannot replace a newer device view`, async (t) => {
    const delayed = deferred<unknown>();
    const gateway = { graphicsTree: () => kind === "close-io" ? delayed.promise : Promise.resolve({ tree: "zenUI::CheatSheetDialog" }), identity: () => delayed.promise, async tapScreen() { throw new Error("must not touch after another view opens"); } } as unknown as GatewayTransport;
    let views!: ReturnType<typeof useQcDeviceViews>; let root!: ReactTestRenderer;
    let destination!: ReturnType<typeof useQcScreenDestination>;
    function Probe() { destination = useQcScreenDestination(); views = useQcDeviceViews({ gateway, snapshot: { deviceName:"QC",setlistKey:"set" }, session: {demo:false,hardwareReady:true,sessionReady:kind === "info"}, destination, workflows: {ioEq:{refresh:()=>delayed.promise}} as never,notice(){},fail(){throw new Error("obsolete failure must be silent");} }); return null; }
    await act(async () => { root=create(React.createElement(Probe)); });
    t.after(async () => { await act(async () => root.unmount()); });
    if (kind === "close-io") await act(async () => { destination.setView("io-overview"); });
    let action!: Promise<void>;
    await act(async () => { action = kind === "close-io" ? views.close() : views.open(kind === "info" ? "info" : "io"); });
    await act(async () => { destination.setView("tuner"); });
    await act(async () => { delayed.resolve(kind === "close-io" ? {tree:"zenUI::CheatSheetDialog"} : {}); await action; });
    assert.equal(destination.view,"tuner");
  });
}
