import assert from "node:assert/strict";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";

// Load the hooks only after the test loader shares React with react-test-renderer.
register("./support/react-singleton-loader.mjs", import.meta.url);
const { createElement } = (await import("react")).default;
const { PresetLifetime } = await import("@qc-remote/core");
const { useQcScreenFollow } = await import("../src/use-qc-screen-follow.ts");
const { useQcScreenDestination } = await import("../src/use-qc-screen-destination.ts");
const { useDeviceSettingsWorkflow } = await import("../src/use-device-settings-workflow.ts");

/**
 * The real hooks wired the way apps/windows App.tsx wires them: a real lifetime, a host session probe and a session key
 * that are rebuilt on EVERY render, the real destination, and the real Settings workflow. The harness in
 * use-qc-screen-follow.test.ts stubs the workflows and passes one fixed probe, so it cannot see a follow that the host's
 * wiring defeats.
 */
function hostedUnit(tree: string) {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const lifetime = new PresetLifetime({ setlistKey: "1_p", presetPosition: 0 });
  const log: string[] = [];
  const gateway = {
    graphicsTree: async () => ({ tree }),
    generalSettings: async () => ({}),
    identity: async () => ({}),
    captureScreen: async () => ({ pngBase64: "" }),
    tapScreen: async () => { log.push("tap"); }, swipeScreen: async () => {}, longTapScreen: async () => {}
  } as never;
  const stream = { current: "stream-1" };
  const seen: { view?: string | null; settingsOpen?: boolean; renders: number } = { renders: 0 };
  function Host() {
    seen.renders += 1;
    const destination = useQcScreenDestination();
    const session = {
      lifetime,
      commandSessionKey: "device",
      sessionKey: `device:7:true:${stream.current}`,
      captureSessionCurrent: () => { const captured = stream.current; return () => stream.current === captured; }
    };
    const common = { ...session, gateway, connected: true, pending: false, setPending() {}, notice: (message: string) => { log.push(`notice:${message}`); }, fail: (error: unknown) => { log.push(`fail:${String(error)}`); } };
    const deviceSettings = useDeviceSettingsWorkflow(common);
    const none = () => undefined;
    useQcScreenFollow({ ...session, gateway, follow: true, available: true, ready: true, destination, notice: common.notice,
      workflows: {
        grid: { close: none, details: undefined }, preset: { closeDirectory: none, openDirectory: none }, deviceSettings,
        tempo: { open: false, close: none, openTempo: none }, tuner: { close: none, openTuner: none }, ioEq: { refresh: none }, modes: { refresh: none }
      } as never });
    seen.view = destination.view; seen.settingsOpen = deviceSettings.open;
    return null;
  }
  return { Host, seen, log, create: () => { let renderer!: ReturnType<typeof create>; act(() => { renderer = create(createElement(Host)); }); return renderer; } };
}
const settle = (ms: number) => act(async () => { await new Promise((resolve) => setTimeout(resolve, ms)); });

test("the app follows the unit into Settings when the host wiring is rebuilt on every render", async () => {
  const host = hostedUnit(["zenUI::RootGraphicsItem", "zenUI::SettingsDialog", "zenUI::SettingsMenu", "zenUI::UpdaterSettings"].join("\n"));
  const renderer = host.create();
  try { await settle(1200); assert.equal(host.seen.settingsOpen, true, JSON.stringify(host.log)); }
  finally { act(() => renderer.unmount()); }
});

test("the app follows the unit into its block editor (the unit's picture) when the host wiring is rebuilt on every render", async () => {
  const host = hostedUnit(["zenUI::RootGraphicsItem", "zenUI::ParameterEditor"].join("\n"));
  const renderer = host.create();
  try { await settle(1200); assert.equal(host.seen.view, "unit-mirror", JSON.stringify(host.log)); }
  finally { act(() => renderer.unmount()); }
});
