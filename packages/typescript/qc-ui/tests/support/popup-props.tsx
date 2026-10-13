import { createElement, type ReactNode } from "react";
import { demoBlockDetails, demoSnapshot } from "@qc-remote/core";
import { QcUnitPicture } from "../../src/qc-unit-screen-mirror.tsx";
import { CorOsSceneCommand } from "../../src/scene-editor.tsx";
import { CorOsDeviceSettings, type DeviceSettingsScreenProps } from "../../src/device-settings-screen.tsx";
import type { QuadCortexSurfaceProps } from "../../src/quad-cortex-surface.tsx";

/** The surface props that open each popup fixture (bundled into the browser page; see popup-fixtures.ts for the registry). */

import { flatPngBase64 } from "./unit-png.ts";
// A real 800x480 PNG, as the unit sends it: a 1x1 pixel lays out differently from a picture with its own shape.
const PNG_800_480 = `data:image/png;base64,${flatPngBase64()}`;
const noop = () => undefined;
const block = (predicate: (name: string, category: string) => boolean) => demoSnapshot.blocks.find((candidate) => candidate.column >= 0 && predicate(candidate.name, String(candidate.category ?? "")))
  ?? demoSnapshot.blocks.find((candidate) => candidate.column >= 0)!;

function editor(pick: (name: string, category: string) => boolean) {
  const details = demoBlockDetails(block(pick), 0);
  return {
    details, drafts: {}, accent: "#7be", activeScene: 0, scenes: demoSnapshot.scenes, bypassed: false, page: 0, onPageChange: noop, onDraftChange: noop, onCommit: noop,
    onToggleSceneMode: noop, onToggleBypass: noop, onSceneSelect: noop, onFootswitchAssignmentStart: noop, onUnassignFootswitch: noop, onContextAction: noop, onClose: noop
  } as unknown as NonNullable<QuadCortexSurfaceProps["parameterEditor"]>;
}

const scenes = demoSnapshot.scenes;
const sceneCommand = (swap: boolean, mirrorsUnit = true): ReactNode => createElement(CorOsSceneCommand, { scenes, sourceScene: 0, swap, pending: false, mirrorsUnit, onSelect: noop, onCancel: noop });
const ports = [[0, "Input 1"], [1, "Input 2"], [2, "Return 1"], [3, "Return 2"], [4, "USB input 5"], [5, "USB input 6"], [6, "USB input 7"], [7, "USB input 8"]] as const;
const models = Array.from({ length: 24 }, (_, index) => ({ id: 100 + index, name: `Model ${index}`, category: ["Amp", "Cab", "Drive", "Delay", "Reverb"][index % 5], type: 0 }));

export const POPUP_PROPS_BY_ID: Record<string, () => Partial<QuadCortexSurfaceProps>> = {
  "preset-menu": () => ({ popup: "grid-menu", onPopupChange: noop }),
  "scene-menu": () => ({ popup: "scene-menu", onPopupChange: noop }),
  "mode-menu": () => ({ popup: "mode-menu", onPopupChange: noop }),
  "input-picker": () => ({ routingPicker: { row: 0, side: "input", options: ports, value: 0, disabled: false, onSelect: noop, onClose: noop } }),
  "output-picker": () => ({ routingPicker: { row: 0, side: "output", options: ports, value: 0, disabled: false, onSelect: noop, onClose: noop } }),
  "add-device-drawer": () => ({ addDevicePicker: { row: 0, column: 3, models, loading: false, pending: false, query: "", onQueryChange: noop, onCategoryChange: noop, pinnedModelIds: [], onSelect: noop, onTogglePinned: noop, onClose: noop } as unknown as QuadCortexSurfaceProps["addDevicePicker"] }),
  "copy-scene": () => ({ sceneCommand: sceneCommand(false) }),
  "swap-scene": () => ({ sceneCommand: sceneCommand(true) }),
  "scene-destination-picker": () => ({ sceneCommand: sceneCommand(false, false) }),
  "scene-label-editor": () => ({ sceneEditor: { sourceScene: 0, setSourceScene: noop, scenes, label: "", setLabel: noop, color: 0, setColor: noop, colors: [], pending: false, saveLabel: noop, saveColor: noop, onClose: noop } as unknown as QuadCortexSurfaceProps["sceneEditor"] }),
  "cpu-monitor": () => ({ cpuMonitor: { cpuLoad: 27, cpuColumns: [], onClose: noop } }),
  "save-as": () => ({ savePreset: { open: true, name: "Preset", disabled: false, onNameChange: noop, onSave: noop, onCancel: noop } }),
  "amp-editor-panel": () => ({ parameterEditor: editor((name, category) => /amp/i.test(category) || /amp/i.test(name)) }),
  "cab-editor-full-screen": () => ({ parameterEditor: editor((name, category) => /cab/i.test(category) || /cab/i.test(name)) }),
  "preset-directory": () => ({ presetDirectory: { open: true, loading: false, disabled: false, onClose: noop, onRefresh: noop, onRecall: noop, onSelectSetlist: noop, onNewSetlist: noop, onPresetAction: noop, onPresetBatchAction: noop, onArrangePreset: noop } as unknown as QuadCortexSurfaceProps["presetDirectory"] }),
  "expression-assignment": () => ({ expressionAssignment: { details: editor(() => true).details, address: { kind: "block" }, pedal: 1, onPedalChange: noop, selected: "bypass", onSelect: noop, pending: false, onAssign: noop, onSetRange: noop, onUnassign: noop, onClose: noop } as unknown as QuadCortexSurfaceProps["expressionAssignment"] }),
  "lane-control-panel": () => ({ laneControlEditor: { details: editor(() => true).details, control: "inputGate", activeScene: 0, scenes, pending: false, onSceneSelect: noop, onContextAction: noop, onClose: noop } as unknown as QuadCortexSurfaceProps["laneControlEditor"] }),
  "unit-mirror-picture": () => ({ screenContent: createElement(QcUnitPicture, { src: PNG_800_480 }) }),
  ...settingsPaneProps()
};

function settingsPaneProps(): Record<string, () => Partial<QuadCortexSurfaceProps>> {
  const rows = { account: ["user", "backup"], system: ["wifi", "updates", "brightness", "power", "volume", "storage", "factory"], device: ["bypass", "scene-bypass", "stomp-bypass", "hold", "swap", "gig-access", "latency", "midi"], support: ["about", "info", "report", "diagnostics", "licenses"] } as const;
  const entries: Record<string, () => Partial<QuadCortexSurfaceProps>> = {};
  for (const [section, names] of Object.entries(rows)) {
    for (const row of names) {
      entries[`settings-${section}-${row}`] = () => ({
        screenContent: createElement(CorOsDeviceSettings, {
          onClose: noop,
          workflow: {
            section, accountRow: section === "account" ? row : "user", systemRow: section === "system" ? row : "wifi", deviceRow: section === "device" ? row : "bypass", supportRow: section === "support" ? row : "about",
            setSection: noop, setAccountRow: noop, setSystemRow: noop, setDeviceRow: noop, setSupportRow: noop, loading: false, announce: noop, identity: undefined, settings: undefined, midiThru: undefined
          } as unknown as DeviceSettingsScreenProps["workflow"]
        })
      });
    }
  }
  return entries;
}
