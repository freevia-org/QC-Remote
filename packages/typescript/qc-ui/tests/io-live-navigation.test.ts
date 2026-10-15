import assert from "node:assert/strict";
import test from "node:test";
import { createRequire, register } from "node:module";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { act, create } from "react-test-renderer";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
const { build } = await import("esbuild");
const work = mkdtempSync(join(tmpdir(), "qc-io-rendered-"));
const outfile = join(work, "io.mjs");
const loaded = await (async () => {
try {
await build({
  stdin: { contents: `export { CorOsIoSettingsScreen, CorOsGlobalEqScreen } from ${JSON.stringify(fileURLToPath(new URL("../src/io-global-eq-screens.tsx", import.meta.url)))};
export { QuadCortexSurface } from ${JSON.stringify(fileURLToPath(new URL("../src/quad-cortex-surface.tsx", import.meta.url)))};
export { demoSnapshot } from "@qc-remote/core";`, resolveDir: fileURLToPath(new URL("../", import.meta.url)), loader: "tsx" },
  outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
  loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl" },
  plugins: [{ name: "renderer-react", setup(builder) {
    builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
  } }]
});
return await import(pathToFileURL(outfile).href);
} catch (error) {
  rmSync(work, { recursive: true, force: true });
  throw error;
}
})();
const { CorOsIoSettingsScreen, CorOsGlobalEqScreen, QuadCortexSurface, demoSnapshot } = loaded;
test.after(() => rmSync(work, { recursive: true, force: true }));
const priorGlobals = new Map(["window", "requestAnimationFrame", "cancelAnimationFrame", "IS_REACT_ACT_ENVIRONMENT"].map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
test.after(() => {
  for (const [key, descriptor] of priorGlobals) {
    if (descriptor) Object.defineProperty(globalThis, key, descriptor);
    else Reflect.deleteProperty(globalThis, key);
  }
});
globalThis.IS_REACT_ACT_ENVIRONMENT = true;
(globalThis as { window?: unknown }).window ??= globalThis;
globalThis.requestAnimationFrame ??= ((callback: FrameRequestCallback) => setTimeout(() => callback(0), 0) as unknown as number);
globalThis.cancelAnimationFrame ??= ((handle: number) => clearTimeout(handle));
async function render(element: unknown) {
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(element as never); });
  return renderer;
}

test("live I/O socket panes never substitute headphones, returns or expression for output writes", async(t)=>{
  const writes:unknown[]=[];
  const io={inputs:[1,2,4,5].map(inputPortId=>({inputPortId,levelDb:0})),outputs:[1,2,4,5,6,7,8,9].map(outputPortId=>({outputPortId,levelDb:0,groundLift:0,muted:false})),expressionPorts:[{expressionPortId:1,level:.7,plugged:true}],headphones:{level:.25,feeds:[{outputPortId:4,level:.3}]},midi:{thru:0},xlr12Linked:true,out34Linked:true};
  const workflow={io,loading:false,pending:false,setOutputPort(...args:unknown[]){writes.push(["output",...args]);},setInputPort(...args:unknown[]){writes.push(["input",...args]);},setMidiThru(enabled:boolean){writes.push(["midi",enabled]);}};
  const root=await render(React.createElement(CorOsIoSettingsScreen,{workflow,onGlobalEq(){},initialView:"headphones"} as never));
  t.after(async()=>{await act(async()=>root.unmount());});
  const range=(label:string)=>root.root.findByProps({"aria-label":label});
  assert.equal(range("Headphone level").props.value,.25);assert.equal(range("Headphone level").props.disabled,true);assert.equal(range("Headphone level").props.onChange,undefined);
  const socket=async(name:string)=>{const icon=root.root.findByProps({"data-io-socket":name});await act(async()=>icon.parent!.props.onClick({target:{closest(){return {getAttribute(){return name;}}},tagName:"I"}}));};
  await socket("ret1");await act(async()=>{range("Return 1 level").props.onChange({target:{value:"2"}});range("Return 2 level").props.onChange({target:{value:"3"}});});
  assert.equal(root.root.findAllByType("input").length,2,"returns expose level only");
  await socket("send1");await act(async()=>{range("Output 8 level").props.onChange({target:{value:"4"}});range("Output 9 level").props.onChange({target:{value:"5"}});});
  await socket("out1");await act(async()=>range("Output 1 level").props.onChange({target:{value:"6"}}));
  await socket("out3");await act(async()=>range("Output 2 level").props.onChange({target:{value:"7"}}));
  await socket("exp1");assert.equal(root.root.findAllByType("input").length,0);assert.match(JSON.stringify(root.toJSON()),/Observed expression port 1/);
  await socket("capture-out");assert.equal(root.root.findAllByType("input").length,0);assert.match(JSON.stringify(root.toJSON()),/Capture Out settings are not available/);
  await socket("midi-in");const on=root.root.findAllByType("button").find(node=>node.children.includes("On"))!;await act(async()=>on.props.onClick());
  assert.deepEqual(writes,[["input",4,2,null,null,null],["input",5,3,null,null,null],["output",8,4,null,null],["output",9,5,null,null],["output",1,6,null,null],["output",2,7,null,null],["midi",true]]);
});

test("live I/O screen navigation forwards the physical tap without duplicating semantic controls", async(t)=>{
  const gestures:unknown[]=[];const order:string[]=[];let localEq=0,semanticWrites=0,closed=0,edgeOpen=0;
  const screenContent=React.createElement(CorOsIoSettingsScreen,{workflow:{io:{inputs:[],outputs:[],xlr12Linked:false},loading:false,pending:false,setOutputPairing(){semanticWrites++;}},onGlobalEq(){order.push("local");localEq++;},onClose(){closed++;}} as never);
  let props={formFactor:{displayName:"QC",controls:[{role:"bank:up"},{role:"bank:down"},{role:"tempo"}]},skin:{className:"test"},snapshot:demoSnapshot,screenView:"io-overview",screenGestureContextKey:"session-A",screenContent,onAction(){},onOpenIoView(){edgeOpen++;},onDeviceScreenGesture(...args:unknown[]){order.push("native");gestures.push(args);}};
  const root=await render(React.createElement(QuadCortexSurface,props as never));
  t.after(async()=>{await act(async()=>root.unmount());});
  const button=root.root.findByProps({className:"io-global-eq"});
  const frame=root.root.findByProps({className:"qc-screen-fixture-root is-canonical-canvas"});
  assert.equal(frame.props.onClick,undefined,"the canvas is not a non-semantic click target; it delegates marked child activations in capture");
  const target={isConnected:true,closest(selector:string){return selector === "button[aria-label^='Close']" ? null : selector.includes("button") || selector === "[data-qc-screen-navigation]" ? this:null;},getAttribute(){return null;},setPointerCapture(){},hasPointerCapture(){return false;},getBoundingClientRect(){return {left:690,top:20,width:20,height:20};}};
  const event={button:0,pointerId:1,clientX:700,clientY:30,target,currentTarget:{contains(){return true;},getBoundingClientRect(){return {left:0,top:0,width:800,height:480};}}};
  await act(async()=>{frame.props.onPointerDown?.(event);frame.props.onPointerUp?.(event);button.props.onClick();});
  assert.equal(localEq,1);
  assert.equal(gestures.length,1,"production screenContent navigation must reach QC as well as local UI");
  await act(async()=>frame.props.onClickCapture({...event,detail:1}));
  assert.equal(gestures.length,1,"pointer click does not repeat pointerup's native tap");
  order.length=0;
  await act(async()=>{frame.props.onClickCapture({...event,detail:0});button.props.onClick();});
  assert.equal(gestures.length,2,"keyboard activation forwards one centre tap");
  assert.deepEqual(order,["native","local"],"keyboard mirroring precedes local pane navigation");
  const semanticTarget={...target,closest(selector:string){return selector === "[data-qc-screen-navigation]"?null:this;}};
  await act(async()=>{frame.props.onPointerDown({...event,target:semanticTarget});frame.props.onPointerUp({...event,target:semanticTarget});root.root.findByProps({"aria-label":"Toggle XLR 1/2 pairing"}).props.onClick();frame.props.onClickCapture({...event,target:semanticTarget,detail:0});});
  assert.equal(gestures.length,2,"semantic writes never receive an additional raw screen mutation");
  assert.equal(semanticWrites,1);
  await act(async()=>root.root.findByProps({"aria-label":"Close I/O Settings"}).props.onClick());
  assert.equal(closed,1);assert.equal(gestures.length,2,"top-level close remains owned by the host workflow");
  const pane={isConnected:true,getAttribute(){return String(root.root.findAll(node=>node.type === "section" && node.props["data-qc-screen-context"] !== undefined)[0].props["data-qc-screen-context"]);}};
  const paneTarget={...target,closest(selector:string){return selector === "[data-qc-screen-context]" ? pane : target.closest(selector);}};
  let oldPaneTimer=()=>{};const nativeTimer=window.setTimeout;
  await act(async()=>{window.setTimeout=((callback:()=>void)=>{oldPaneTimer=callback;return 124;}) as typeof window.setTimeout;try{frame.props.onPointerDown({...event,target:paneTarget});}finally{window.setTimeout=nativeTimer;}});
  const midiSocket=root.root.findByProps({"data-io-socket":"midi-in"});
  await act(async()=>midiSocket.parent!.props.onClick({target:{closest(){return null;},tagName:"I"}}));
  await act(async()=>{oldPaneTimer();frame.props.onPointerUp({...event,target:paneTarget});});
  assert.equal(gestures.length,2,"inner I/O navigation retires held raw taps even when outer screenView is unchanged");
  await act(async()=>frame.props.onPointerDown({...event,target:paneTarget}));pane.isConnected=false;
  await act(async()=>frame.props.onPointerUp({...event,target:paneTarget}));assert.equal(gestures.length,2,"a detached pane cannot retain an admitted gesture just because its last attribute is unchanged");pane.isConnected=true;
  let firstTimer=()=>{};let secondTimer=()=>{};let timerCount=0;
  await act(async()=>{window.setTimeout=((callback:()=>void)=>{if(timerCount++===0)firstTimer=callback;else secondTimer=callback;return 130+timerCount;}) as typeof window.setTimeout;try{frame.props.onPointerDown(event);frame.props.onPointerDown({...event,pointerId:2});}finally{window.setTimeout=nativeTimer;}});
  await act(async()=>firstTimer());assert.equal(gestures.length,2,"replacement pointerdown retires the previous long-tap ownership");
  await act(async()=>frame.props.onPointerCancel({...event,pointerId:2}));await act(async()=>secondTimer());assert.equal(gestures.length,2);
  const oldUp=frame.props.onPointerUp,oldKeyboard=frame.props.onClickCapture;
  await act(async()=>frame.props.onPointerDown(event));
  await act(async()=>{props={...props,screenGestureContextKey:"session-B"};root.update(React.createElement(QuadCortexSurface,props as never));});
  await act(async()=>{oldUp(event);oldKeyboard({...event,detail:0});});
  assert.equal(gestures.length,2,"a pointer gesture and retained keyboard callback from the old device cannot dispatch to its replacement");
  await act(async()=>{props={...props,screenGestureContextKey:"session-A"};root.update(React.createElement(QuadCortexSurface,props as never));});
  let queuedLongTap:()=>void=()=>{throw new Error("timer not scheduled");};
  const realSetTimeout=window.setTimeout;
  await act(async()=>{window.setTimeout=((callback:()=>void)=>{queuedLongTap=callback;return 123;}) as typeof window.setTimeout;try{root.root.findByProps({className:"qc-screen-fixture-root is-canonical-canvas"}).props.onPointerDown(event);}finally{window.setTimeout=realSetTimeout;}});
  await act(async()=>{props={...props,screenGestureContextKey:"session-B"};root.update(React.createElement(QuadCortexSurface,props as never));});
  await act(async()=>{props={...props,screenGestureContextKey:"session-A"};root.update(React.createElement(QuadCortexSurface,props as never));});
  await act(async()=>queuedLongTap());
  assert.equal(gestures.length,2,"an already queued retired timer cannot resurrect when an equal context returns");
  await act(async()=>oldKeyboard({...event,detail:0}));
  assert.equal(gestures.length,2,"a retained keyboard callback also stays retired after session ABA");
  let back=0;
  await act(async()=>{props={...props,screenView:"global-eq",screenContent:React.createElement(CorOsGlobalEqScreen,{workflow:{loading:false,pending:false,globalEq:{status:"unavailable"}},onBack(){back++;}} as never)};root.update(React.createElement(QuadCortexSurface,props as never));});
  const eqFrame=root.root.findByProps({className:"qc-screen-fixture-root is-canonical-canvas"});
  const closeTarget={...target,closest(){return this;},getAttribute(){return "Close Global EQ";}};
  await act(async()=>{eqFrame.props.onPointerDown({...event,target:closeTarget});eqFrame.props.onPointerUp({...event,target:closeTarget});root.root.findByProps({"aria-label":"Close Global EQ"}).props.onClick();});
  assert.equal(back,1);assert.equal(gestures.length,3,"actual global-eq destination forwards its nested close tap");
  await act(async()=>{props={...props,screenView:"grid"};root.update(React.createElement(QuadCortexSurface,props as never));});
  const bezel=()=>root.root.findByProps({className:"qc-screen-bezel is-grid-edge-gesture-surface"});
  const edgeTarget={...event.currentTarget,setPointerCapture(){},hasPointerCapture(){return true;},releasePointerCapture(){}};
  const edgeEvent={...event,currentTarget:edgeTarget,preventDefault(){},stopPropagation(){}};
  await act(async()=>{bezel().props.onPointerDownCapture(edgeEvent);bezel().props.onPointerMoveCapture({...edgeEvent,clientY:100});});
  const obsoleteEdge=bezel().props.onPointerUpCapture;
  await act(async()=>{props={...props,screenGestureContextKey:"session-C"};root.update(React.createElement(QuadCortexSurface,props as never));});
  await act(async()=>obsoleteEdge({...edgeEvent,clientY:100}));assert.equal(edgeOpen,0);
  await act(async()=>{bezel().props.onPointerDownCapture(edgeEvent);bezel().props.onPointerMoveCapture({...edgeEvent,clientY:100});bezel().props.onPointerUpCapture({...edgeEvent,clientY:100});});
  assert.equal(edgeOpen,1,"new session still accepts its own edge swipe");
});
