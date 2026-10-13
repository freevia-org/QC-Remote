import assert from "node:assert/strict";
import test from "node:test";
import {register} from "node:module";
import {act,create,type ReactTestRenderer} from "react-test-renderer";
register("./support/react-singleton-loader.mjs",import.meta.url);
const React=(await import("react")).default;
const {PresetLifetime,demoSnapshot}=await import("@qc-remote/core");
const {useSceneWorkflow}=await import("../src/use-scene-workflow.ts");
globalThis.IS_REACT_ACT_ENVIRONMENT=true;

function deferred<T>() {let resolve!:(value:T)=>void;const promise=new Promise<T>(done=>{resolve=done;});return {promise,resolve};}
function host() {
  let epoch=0;
  let snapshot={...demoSnapshot,presetIdentity:"preset-A",stateVersion:{session:"native-A",revision:1}};
  const lifetime=new PresetLifetime(snapshot);
  const calls:unknown[][]=[];
  const notices:string[]=[];
  let copy=async()=>({detail:"Copied"});
  const gateway={async copyScene(...args:unknown[]){calls.push(["copy",...args]);return copy();},async setSceneLabel(...args:unknown[]){calls.push(["label",...args]);return {detail:"Label updated"};},async setSceneColor(...args:unknown[]){calls.push(["color",...args]);return {detail:"Color updated"};}};
  let workflow!:ReturnType<typeof useSceneWorkflow>;
  const render=()=>React.createElement(Probe);
  function Probe(){const [pending,setPending]=React.useState(false);workflow=useSceneWorkflow({gateway:gateway as never,snapshot,presetLifetime:lifetime,commandSessionKey:"wire-A",captureSessionCurrent(){const held=epoch;return()=>held===epoch;},connected:true,pending,setPending,reconcile(){},recordHistory(){},notice:(value)=>notices.push(value),fail:(error)=>{throw error;}});return null;}
  let root!:ReactTestRenderer;act(()=>{root=create(render());});
  return {root,calls,notices,lifetime,get workflow(){return workflow;},setCopy:(next:typeof copy)=>{copy=next;},retireHost:()=>{epoch++;},replacePreset:()=>{snapshot={...snapshot,presetIdentity:"preset-B",scenes:["New preset",...snapshot.scenes.slice(1)]};lifetime.invalidate();},rerender:()=>act(()=>root.update(render()))};
}

test("scene modal drafts and retained writes retire on same-address preset and host replacement",async(t)=>{
  const h=host();t.after(()=>act(()=>h.root.unmount()));
  act(()=>{h.workflow.openLabelColor(0);h.workflow.openCommandForScene(0,false);});
  act(()=>h.workflow.setLabel("Old draft"));
  const old=h.workflow;
  act(()=>h.replacePreset());h.rerender();
  assert.equal(h.workflow.labelColorOpen,false);assert.equal(h.workflow.command,null);
  await act(async()=>{await old.saveLabel();await old.chooseDestination(1);});assert.deepEqual(h.calls,[]);
  act(()=>h.workflow.openLabelColor(0));assert.equal(h.workflow.label,"New preset");
  act(()=>h.workflow.setLabel("Current draft"));
  await act(async()=>{await h.workflow.saveLabel();});assert.equal(h.calls.length,1);assert.equal(h.calls[0][2],"Current draft");
  const current=h.workflow;h.retireHost();
  await act(async()=>{await current.saveLabel();});assert.equal(h.calls.length,1,"physical retirement refuses before rerender");
});

test("late scene copy and obsolete dismissals cannot close successor command or label dialogs",async(t)=>{
  const h=host();t.after(()=>act(()=>h.root.unmount()));
  const waiting=deferred<{detail:string}>();h.setCopy(()=>waiting.promise);
  act(()=>h.workflow.openCommandForScene(0,false));const oldCommand=h.workflow;
  let started!:Promise<void>;act(()=>{started=h.workflow.chooseDestination(1);});
  act(()=>h.workflow.openUnitCommand(true,2));
  act(()=>oldCommand.closeCommand());assert.equal(h.workflow.command,"swap");
  await act(async()=>{waiting.resolve({detail:"Copied"});await started;});
  assert.equal(h.workflow.command,"swap");assert.equal(h.workflow.sourceScene,2);assert.equal(h.calls.length,1);
  act(()=>h.workflow.openLabelColor(0));act(()=>h.workflow.setLabel("Cancelled draft"));const oldLabel=h.workflow;
  act(()=>h.workflow.closeLabelColor());act(()=>h.workflow.openLabelColor(1));
  act(()=>{oldLabel.closeLabelColor();oldLabel.setLabel("Stale draft");});
  await act(async()=>{await oldLabel.saveLabel();});
  assert.equal(h.workflow.labelColorOpen,true);assert.equal(h.workflow.label,demoSnapshot.scenes[1]);assert.equal(h.calls.length,1);
});