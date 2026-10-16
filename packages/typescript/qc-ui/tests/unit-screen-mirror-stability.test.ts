import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { flatPngBase64 } from "./support/unit-png.ts";

// Load the component only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
register("./support/tsx-css-loader.mjs", import.meta.url);
const { createElement, useState } = (await import("react")).default;
const { PresetLifetime } = await import("@qc-remote/core");
const { QcUnitScreenMirror } = await import("../src/qc-unit-screen-mirror.tsx");

/**
 * The host (apps/windows App.tsx) builds `captureSessionCurrent` inline, so every host render hands the mirror a NEW
 * function. The mirror must read it through a ref: restarting its capture loop on each host render cancels every
 * in-flight capture, and a host that renders more often than the unit answers never shows a picture.
 */
test("the mirrored picture appears while the host re-renders faster than the unit answers a capture", async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const lifetime = new PresetLifetime({ setlistKey: "1_p", presetPosition: 0 });
  const png = flatPngBase64();
  let captures = 0;
  const gateway = { captureScreen: () => new Promise<{ pngBase64: string }>((resolve) => { captures += 1; setTimeout(() => resolve({ pngBase64: png }), 40); }) };
  let rerender!: () => void;
  function Host() {
    const [, setTick] = useState(0);
    rerender = () => setTick((tick) => tick + 1);
    return createElement(QcUnitScreenMirror, { gateway, lifetime, intervalMs: 1000, captureSessionCurrent: () => () => true });
  }
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(createElement(Host)); });
  try {
    const started = Date.now();
    while (Date.now() - started < 400) {
      await act(async () => { rerender(); await new Promise((resolve) => setTimeout(resolve, 10)); });
    }
    assert.ok(renderer.root.findAllByType("img").length > 0, `no picture after 400 ms of host renders (${captures} captures started)`);
    assert.ok(captures <= 2, `the capture loop restarted ${captures} times for one session`);
  } finally { act(() => renderer.unmount()); }
});
