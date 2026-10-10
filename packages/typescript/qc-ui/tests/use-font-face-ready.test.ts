import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import type { ReactElement } from "react";
import type { ReactTestRenderer } from "react-test-renderer";

// Load React, the renderer and the hook only after the test loader shares React with react-test-renderer;
// a static import of any of them would create a second React copy and a second dispatcher.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement, StrictMode } = (await import("react")).default;
const { act, create } = await import("react-test-renderer");
const { useFontFaceReady } = await import("../src/use-font-face-ready.ts");

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const FONT = '800 69px "QC CorOS IBM Plex Sans", sans-serif';
const TEXT = "32Hpyquadcortex scratch";
const OTHER_TEXT = "32HQC MCP TEST";
// react-test-renderer 19 announces its own deprecation through console.error on create(); that is not under test.
const KNOWN_RENDERER_NOTICE = "react-test-renderer is deprecated";

type Fonts = unknown;
interface Harness {
  mount(element: ReactElement): Promise<ReactTestRenderer>;
  settle(): Promise<void>;
  errors: unknown[][];
  unhandled: unknown[];
  unexpectedErrors(): unknown[][];
}

interface Held { promise: Promise<unknown[]>; resolve: (value: unknown[]) => void }
const deferred = (): Held => {
  let resolve!: (value: unknown[]) => void;
  const promise = new Promise<unknown[]>((res) => { resolve = res; });
  return { promise, resolve };
};

/**
 * Stubs `document` for one test and ALWAYS tears down in the right order: every mounted renderer is unmounted
 * (inside act) first, and only then are `document`, console.error and the unhandledRejection listener restored.
 */
async function withHarness(fonts: Fonts | "no-document", run: (harness: Harness) => Promise<void>): Promise<void> {
  const mounted: ReactTestRenderer[] = [];
  const errors: unknown[][] = [];
  const unhandled: unknown[] = [];
  const onUnhandled = (reason: unknown) => { unhandled.push(reason); };
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const originalError = console.error;
  process.on("unhandledRejection", onUnhandled);
  console.error = (...args: unknown[]) => { errors.push(args); };
  Object.defineProperty(globalThis, "document", { configurable: true, writable: true, value: fonts === "no-document" ? undefined : { fonts } });
  const settle = async () => {
    await act(async () => {
      for (let index = 0; index < 5; index += 1) await Promise.resolve();
      await new Promise((resolve) => setTimeout(resolve, 0));
    });
  };
  try {
    await run({
      async mount(element) {
        let renderer!: ReactTestRenderer;
        await act(async () => { renderer = create(element); });
        mounted.push(renderer);
        return renderer;
      },
      settle,
      errors,
      unhandled,
      unexpectedErrors: () => errors.filter((args) => !String(args[0]).includes(KNOWN_RENDERER_NOTICE))
    });
  } finally {
    for (const renderer of mounted.reverse()) {
      try { await act(async () => { renderer.unmount(); }); } catch { /* already unmounted by the test */ }
    }
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else delete (globalThis as { document?: unknown }).document;
    console.error = originalError;
    process.off("unhandledRejection", onUnhandled);
  }
}

/** A component that records the hook's return value on every render. */
function probe(log: boolean[]) {
  return function Probe({ text, enabled }: { text: string; enabled?: boolean }) {
    const ready = useFontFaceReady(FONT, text, enabled);
    log.push(ready);
    return createElement("span", null, String(ready));
  };
}

test("a render with ready=true follows the held font load, with no manual re-render", async () => {
  const held = deferred();
  const loads: Array<[string, string]> = [];
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: (spec: string, text: string) => { loads.push([spec, text]); return held.promise; } }, async (h) => {
    await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
    assert.deepEqual(log, [false], "first render is not ready");
    assert.deepEqual(loads, [[FONT, TEXT]], "waits for exactly the measured spec with the real text");
    await h.settle();
    assert.deepEqual(log, [false], "still not ready while the load is held");
    await act(async () => { held.resolve([{}]); await held.promise; });
    assert.deepEqual(log, [false, true], "exactly one extra render, now ready");
    await h.settle();
    assert.deepEqual(log, [false, true], "no further renders");
    assert.equal(loads.length, 1, "no further loads");
    assert.deepEqual(h.unexpectedErrors(), []);
    assert.deepEqual(h.unhandled, []);
  });
});

test("a rejected load leaves ready false, renders no more, and leaves no unhandled rejection", async () => {
  let loads = 0;
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: () => { loads += 1; return Promise.reject(new Error("NetworkError")); } }, async (h) => {
    await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
    await h.settle();
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.deepEqual(log, [false]);
    assert.equal(loads, 1);
    assert.deepEqual(h.unhandled, []);
    assert.deepEqual(h.unexpectedErrors(), []);
  });
});

test("resolving after unmount neither throws, logs, nor renders (the cleanup flag is not observable beyond this)", async () => {
  // React 18+ drops a state update on an unmounted component silently: no warning is emitted and nothing renders,
  // so no assertion here can observe the setState itself. What is proven: the held promise settles after unmount
  // without an exception, an unhandled rejection or any console.error other than the renderer's own notice, and
  // the probe is not rendered again. The `active` flag itself is covered by review, not by this test.
  const held = deferred();
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: () => held.promise }, async (h) => {
    const renderer = await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
    assert.deepEqual(log, [false]);
    await act(async () => { renderer.unmount(); });
    await act(async () => { held.resolve([{}]); await held.promise; });
    await h.settle();
    assert.deepEqual(log, [false], "no render after unmount");
    assert.deepEqual(h.unexpectedErrors(), []);
    assert.deepEqual(h.unhandled, []);
  });
});

test("changed text re-arms once: a second load, one extra ready render, and no loop", async () => {
  const held: Held[] = [];
  const loads: string[] = [];
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: (_spec: string, text: string) => { loads.push(text); const d = deferred(); held.push(d); return d.promise; } }, async (h) => {
    const renderer = await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
    await act(async () => { held[0].resolve([{}]); await held[0].promise; });
    assert.deepEqual(log, [false, true]);
    await act(async () => { renderer.update(createElement(Probe, { text: OTHER_TEXT, enabled: true })); });
    assert.deepEqual(log, [false, true, false], "new text is not ready until its own load settles");
    assert.deepEqual(loads, [TEXT, OTHER_TEXT]);
    await act(async () => { held[1].resolve([{}]); await held[1].promise; });
    assert.deepEqual(log, [false, true, false, true]);
    await h.settle();
    await h.settle();
    assert.deepEqual(log, [false, true, false, true], "no loop");
    assert.equal(loads.length, 2, "no load storm");
    assert.deepEqual(h.unexpectedErrors(), []);
  });
});

test("a load that settles for superseded text does not mark the latest text ready", async () => {
  const held: Held[] = [];
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: () => { const d = deferred(); held.push(d); return d.promise; } }, async (h) => {
    const renderer = await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
    await act(async () => { renderer.update(createElement(Probe, { text: OTHER_TEXT, enabled: true })); });
    await act(async () => { held[0].resolve([{}]); await held[0].promise; });
    await h.settle();
    assert.equal(log.includes(true), false, "the stale load did not make the new text ready");
    await act(async () => { held[1].resolve([{}]); await held[1].promise; });
    assert.equal(log[log.length - 1], true);
  });
});

test("enabled=false never calls the font API", async () => {
  let loads = 0;
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: () => { loads += 1; return Promise.resolve([]); } }, async (h) => {
    await h.mount(createElement(Probe, { text: TEXT, enabled: false }));
    await h.settle();
    assert.equal(loads, 0);
    assert.deepEqual(log, [false]);
  });
});

test("an absent document, absent document.fonts, missing fonts.load, or a throwing load stay not-ready without error", async () => {
  let throwingLoads = 0;
  const cases: Array<[string, Fonts | "no-document", () => number]> = [
    ["no document", "no-document", () => 0],
    ["document.fonts undefined", undefined, () => 0],
    ["fonts.load missing", {}, () => 0],
    ["load throws synchronously", { load: () => { throwingLoads += 1; throw new SyntaxError("bad spec"); } }, () => throwingLoads]
  ];
  for (const [label, fonts, calls] of cases) {
    const log: boolean[] = [];
    const Probe = probe(log);
    await withHarness(fonts, async (h) => {
      await h.mount(createElement(Probe, { text: TEXT, enabled: true }));
      await h.settle();
      assert.deepEqual(log, [false], label);
      assert.deepEqual(h.unhandled, [], label);
      assert.deepEqual(h.unexpectedErrors(), [], label);
    });
    if (label === "load throws synchronously") assert.equal(calls(), 1, "the throwing load was attempted once and swallowed");
  }
});

test("under StrictMode the double effect settles to ready with at most two loads", async () => {
  let loads = 0;
  const log: boolean[] = [];
  const Probe = probe(log);
  await withHarness({ load: () => { loads += 1; return Promise.resolve([{}]); } }, async (h) => {
    await h.mount(createElement(StrictMode, null, createElement(Probe, { text: TEXT, enabled: true })));
    await h.settle();
    assert.ok(loads >= 1 && loads <= 2);
    assert.equal(log[log.length - 1], true);
    assert.deepEqual(h.unexpectedErrors(), []);
  });
});
