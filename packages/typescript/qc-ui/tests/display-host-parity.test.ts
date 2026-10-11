import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { bundleModule } from "./support/display-page.ts";

/*
 * The owner's rule: the display is identical on Windows and Android. Both
 * hosts hand the shared display their own adapters (transports, connection
 * state, their own entry points); these tests mount the real display with two
 * such adapters and require the output to be the same for every screen, so a
 * host-specific value can never change what the display draws.
 */

type Flavour = "windows" | "android";
interface Scenario { popup?: "grid-menu" | "scene-menu" | "mode-menu" | null; view: string | null; open?: "deviceSettings" | "tempo"; presentation?: "hardware" | "display"; cpuOpen?: boolean; information?: boolean }
type Renderer = { render(flavour: Flavour, scenario: Scenario): Promise<string>; surfaceKeys(flavour: Flavour): Promise<string[]> };

const workDirectory = mkdtempSync(join(tmpdir(), "qc-display-parity-"));
const entry = fileURLToPath(new URL("./support/display-host-entry.tsx", import.meta.url));
const { module } = await bundleModule<Renderer>(workDirectory, "parity", `export * from ${JSON.stringify(entry)};`);
test.after(() => rmSync(workDirectory, { recursive: true, force: true }));

const SCREENS: [string, Scenario][] = [
  ["live grid", { view: null }],
  ["tuner", { view: "tuner" }],
  ["looper", { view: "looper-editor" }],
  ["modes configuration", { view: "modes" }],
  ["preset midi out", { view: "midi-out" }],
  ["global eq", { view: "global-eq" }],
  ["io settings", { view: "io-overview" }],
  ["cpu monitor over the grid", { view: "cpu-monitor" }],
  ["gig view", { view: "gig" }],
  ["device information", { view: "settings-info", information: true }],
  ["unit mirror", { view: "unit-mirror" }],
  ["preset menu popup", { view: null, popup: "grid-menu" }],
  ["scene menu popup", { view: null, popup: "scene-menu" }],
  ["device settings", { view: null, open: "deviceSettings" }],
  ["tempo", { view: null, open: "tempo" }]
];

test("the display draws the same markup for a Windows-like and an Android-like host, on every screen", async () => {
  const seen = new Map<string, string>();
  for (const [name, scenario] of SCREENS) {
    for (const presentation of ["hardware", "display"] as const) {
      const windows = await module.render("windows", { ...scenario, presentation });
      const android = await module.render("android", { ...scenario, presentation });
      assert.ok(windows.length > 2000, `${name} (${presentation}) rendered a display`);
      assert.equal(android, windows, `${name} (${presentation}) must not depend on which host mounted it`);
      if (presentation === "display") seen.set(name, windows);
    }
  }
  // Guard against a fake that renders the same thing for every screen: each screen is its own picture.
  const distinct = new Set(seen.values());
  assert.equal(distinct.size, seen.size, "every screen draws differently from every other");
});

test("the screens that must come from the live workflows are really in the markup", async () => {
  const markup = async (scenario: Scenario) => module.render("windows", { ...scenario, presentation: "display" });
  assert.match(await markup({ view: "tuner" }), /coros-tuner|tuner/i);
  assert.match(await markup({ view: "modes" }), /coros-modes-configuration/);
  assert.match(await markup({ view: "looper-editor" }), /coros-looper/);
  assert.match(await markup({ view: "cpu-monitor" }), /CPU Monitor/);
  assert.match(await markup({ view: "settings-info", information: true }), /S-1/, "the Device Information screen shows the identity the host's views read");
});

test("the assembled display sets exactly the display props of the surface, and no host can add to them", async () => {
  const source = readFileSync(fileURLToPath(new URL("../src/quad-cortex-surface.tsx", import.meta.url)), "utf8").replaceAll("\r\n", "\n");
  const body = source.slice(source.indexOf("export interface QcDisplaySurfaceProps {"), source.indexOf("export type QuadCortexSurfaceProps"));
  const declared = [...body.matchAll(/^  (\w+)\??:/gm)].map((match) => match[1]).sort();
  assert.ok(declared.length > 30, "the display props were found in the interface");
  assert.deepEqual(await module.surfaceKeys("windows"), declared, "createQcDeviceDisplay must set every display prop, so adding one to the surface fails here until the display assembles it");
  assert.deepEqual(await module.surfaceKeys("android"), declared);
  const host = source.slice(source.indexOf("export interface QcHostSurfaceProps {"), source.indexOf("/** Everything the display shows and does."));
  assert.deepEqual([...host.matchAll(/^  (\w+)\??:/gm)].map((match) => match[1]).sort(), ["formFactor", "onAction", "onMasterVolumeInteractionEnd", "onMasterVolumeInteractionStart", "presentation", "skin"], "a host chooses only the frame, the hardware controls and how hardware input reaches its workflows");
});
