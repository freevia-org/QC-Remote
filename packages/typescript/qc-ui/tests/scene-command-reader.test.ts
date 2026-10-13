import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { classifyQcGraphicsTree } from "../src/use-adaptive-screen-polling.ts";
import { decodeUnitFrame, sceneCommandVerbFromFrame, sceneCommandVerbFromLuma, sceneIndexFromTree } from "../src/scene-command-reader.ts";
import { titleFrame } from "./support/scene-command-frame.ts";

/** The unit's tree with its Copy Scene / Swap Scene dialog open on 4.1.0 (the same bytes for both commands). */
const TREE = readFileSync(new URL("./support/copy-scene-unit-tree.txt", import.meta.url), "utf8");

test("the real tree is the Copy/Swap Scene screen, whose only text is CANCEL", () => {
  assert.equal(classifyQcGraphicsTree(TREE), "copy-scene");
  const dialog = TREE.slice(TREE.indexOf("zenUI::CopySceneDialog"));
  assert.deepEqual([...dialog.matchAll(/text\s*:\s*'([^']*)'/g)].map((match) => match[1]), ["CANCEL"], "no title and no prompt in the tree: they are read from the frame");
});

test("the source scene is the letter in the Grid's scene indicator", () => {
  assert.equal(sceneIndexFromTree(TREE), 0);
  for (const [letter, index] of [["B", 1], ["D", 3], ["H", 7]] as const) assert.equal(sceneIndexFromTree(TREE.replace("text : 'A'", `text : '${letter}'`)), index);
  assert.equal(sceneIndexFromTree(TREE.replace("text : 'A'", "text : 'Z'")), undefined);
  assert.equal(sceneIndexFromTree(TREE.replace("zenUI::SceneIndicator", "zenUI::Other")), undefined);
  assert.equal(sceneIndexFromTree("zenUI::Grid\nzenUI::CopySceneDialog\n  zenUI::Button\n    text : 'CANCEL'"), undefined);
  // The label before the indicator, which holds a line break, is not mistaken for it.
  assert.equal(sceneIndexFromTree("zenUI::SceneIndicator\n  zenUI::GraphicsItem\n    text : 'B'"), 1);
});

test("Copy and Swap are told apart by the title's first letter, wherever a scene letter moves the centred title", () => {
  for (const x of [333, 337, 339, 342, 346]) {
    assert.equal(sceneCommandVerbFromLuma({ width: 800, height: 480, luma: titleFrame("copy", x) }), "copy", `copy at ${x}`);
    assert.equal(sceneCommandVerbFromLuma({ width: 800, height: 480, luma: titleFrame("swap", x) }), "swap", `swap at ${x}`);
  }
});

test("a frame that does not show a readable title is never guessed", async () => {
  assert.equal(sceneCommandVerbFromLuma({ width: 800, height: 480, luma: titleFrame("none") }), undefined, "no title");
  assert.equal(sceneCommandVerbFromLuma({ width: 800, height: 480, luma: titleFrame("swap", 339, false) }), undefined, "a frame that still shows the Grid is not the dialog, whatever it holds where the title goes");
  assert.equal(sceneCommandVerbFromLuma({ width: 400, height: 240, luma: new Uint8Array(400 * 240) }), undefined, "not the unit's shape");
  const half = titleFrame("copy");
  for (let y = 178; y < 182; y += 1) half[y * 800 + 339] = 16;
  assert.equal(sceneCommandVerbFromLuma({ width: 800, height: 480, luma: half }), undefined, "between the two");
  assert.equal(await sceneCommandVerbFromFrame({ pngBase64: "x" }, async () => undefined), undefined, "an undecodable frame");
  assert.equal(await sceneCommandVerbFromFrame({ pngBase64: "x" }, async () => ({ width: 800, height: 480, luma: titleFrame("swap") })), "swap");
});

test("browser scene-frame decoding closes its bitmap on success and every post-allocation failure", async () => {
  const bitmapDescriptor = Object.getOwnPropertyDescriptor(globalThis,"createImageBitmap");
  const canvasDescriptor = Object.getOwnPropertyDescriptor(globalThis,"OffscreenCanvas");
  let closed = 0;
  let mode: "success" | "missing" | "failure" = "success";
  Object.defineProperty(globalThis,"createImageBitmap",{configurable:true,value:async()=>({width:1,height:1,close(){closed++;}})});
  Object.defineProperty(globalThis,"OffscreenCanvas",{configurable:true,value:class {
    getContext() {
      if(mode === "missing") return null;
      return {drawImage(){if(mode === "failure") throw new Error("decode failed");},getImageData(){return {data:new Uint8Array([255,255,255,255])};}};
    }
  }});
  try {
    assert.equal((await decodeUnitFrame("AA=="))?.luma[0],255); assert.equal(closed,1);
    mode="missing"; assert.equal(await decodeUnitFrame("AA=="),undefined); assert.equal(closed,2);
    mode="failure"; assert.equal(await decodeUnitFrame("AA=="),undefined); assert.equal(closed,3);
  } finally {
    if(bitmapDescriptor) Object.defineProperty(globalThis,"createImageBitmap",bitmapDescriptor); else Reflect.deleteProperty(globalThis,"createImageBitmap");
    if(canvasDescriptor) Object.defineProperty(globalThis,"OffscreenCanvas",canvasDescriptor); else Reflect.deleteProperty(globalThis,"OffscreenCanvas");
  }
});