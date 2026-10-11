import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";

/*
 * One owner for the on-device display. A host mounts QcDeviceDisplay with a
 * model from createQcDeviceDisplay and chooses only the frame and its hardware
 * controls. These tests fail the moment either host (or the package surface)
 * lets display logic back in. The Windows/Android parity of the output itself
 * is display-host-parity.test.ts; the CSS ban is display-single-source.test.ts.
 */

const read = (relative: string) => readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8").replaceAll("\r\n", "\n");
const code = (relative: string) => read(relative).replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
const hosts = [["windows", code("../../../../apps/windows/src/App.tsx")], ["android", code("../../../../apps/android/src/App.tsx")]] as const;
const index = read("../src/index.ts");

test("neither host assembles the display: no surface, no screen, no display state of its own", () => {
  const forbidden = /\bQuadCortexSurface\b|\bCorOs\w*Screen\b|\bCorOsCpuMonitor\w*\b|\bCorOsSceneCommand\b|\bLooperXScreen\b|\bModesConfigurationScreen\b|\bPresetMidiOutEditor\b|setScreenDestination|\bcpuColumns\b|\bcpuLoad\b|\bdeviceInformation\b|screenContent|\bscreenView=/i;
  for (const [name, source] of hosts) assert.doesNotMatch(source, forbidden, `${name} must leave the display to the shared code`);
});

test("both hosts mount the shared display from one assembled model", () => {
  for (const [name, source] of hosts) {
    assert.equal((source.match(/createQcDeviceDisplay\(/g) ?? []).length, 1, `${name} assembles the display exactly once`);
    assert.equal((source.match(/<QcDeviceDisplay /g) ?? []).length, 1, `${name} mounts the display exactly once`);
    assert.equal((source.match(/<QcDeviceDisplayOverlay /g) ?? []).length, 1, `${name} mounts the display's overlay exactly once`);
    // The mount takes the model and only the host's own frame and hardware controls.
    const mount = source.slice(source.indexOf("<QcDeviceDisplay "), source.indexOf("/>", source.indexOf("<QcDeviceDisplay ")));
    const props = [...mount.matchAll(/\s([a-zA-Z]+)=/g)].map((match) => match[1]).sort();
    const allowed = ["display", "formFactor", "onAction", "onMasterVolumeInteractionEnd", "onMasterVolumeInteractionStart", "presentation", "skin"];
    assert.deepEqual(props.filter((prop) => !allowed.includes(prop)), [], `${name} passes no display prop to the display`);
  }
});

test("the package does not export the surface or any device screen for a host to assemble", () => {
  assert.doesNotMatch(index, /\bQuadCortexSurface\b/);
  assert.doesNotMatch(index, /\b(?:CorOsDeviceSettings|CorOsTempoScreen|CorOsTunerScreen|LooperXScreen|ModesConfigurationScreen|PresetMidiOutEditor|CorOsIoSettingsScreen|CorOsGlobalEqScreen|CorOsSceneCommand)\b/);
  assert.doesNotMatch(index, /\b(?:qcDeviceScreenContent|resolveQcDeviceScreen|qcContextActionHandler|QC_DISABLED_CONTEXT_ACTIONS|qcParameterEditorLeds)\b/, "the pieces the display is assembled from stay internal");
  assert.match(index, /createQcDeviceDisplay, QcDeviceDisplay, QcDeviceDisplayOverlay/);
});

test("the host-facing surface props are only the frame and the hardware controls", () => {
  const surface = read("../src/quad-cortex-surface.tsx");
  assert.match(surface, /export type QuadCortexSurfaceProps = QcHostSurfaceProps & QcDisplaySurfaceProps;/);
  // QcDisplaySurfaceProps is built only by createQcDeviceDisplay (display-host-parity.test.ts checks it sets every key).
  const users = ["../../../../apps/windows/src/App.tsx", "../../../../apps/android/src/App.tsx"].map(read).join("\n");
  assert.doesNotMatch(users, /QcDisplaySurfaceProps|QuadCortexSurfaceProps/);
});
