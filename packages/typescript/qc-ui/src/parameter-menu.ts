import type { BlockDetails } from "@qc-remote/client";

export type ParameterEditorContextAction =
  | "save-device-preset" | "change-device" | "copy-device" | "paste-device"
  | "reset-defaults" | "expression"
  | "assign-looper-actions" | "mute-bypass" | "model-update" | "legacy-version" | "remove";

export interface ParameterContextMenuItem {
  action: ParameterEditorContextAction;
  label: string;
  icon: "save" | "change" | "copy" | "paste" | "reset" | "expression" | "looper" | "mute" | "model-update" | "model-downgrade" | "remove";
  disabled?: boolean;
}

export function parameterContextMenuItems(details: Pick<BlockDetails, "name" | "category" | "modelId">, clipboardModelId?: number): ParameterContextMenuItem[] {
  const category = details.category.trim().toLowerCase();
  const supportsDevicePresets = !/(?:neural\s*capture|input|output|splitter|mixer)/i.test(category);
  const looper = /looper/i.test(`${details.category} ${details.name}`);
  return [
    { action: "change-device", label: "Change device", icon: "change" },
    { action: "copy-device", label: "Copy device", icon: "copy" },
    { action: "paste-device", label: "Paste device", icon: "paste", disabled: clipboardModelId !== details.modelId },
    { action: "reset-defaults", label: "Reset to defaults", icon: "reset" },
    ...(supportsDevicePresets ? [{ action: "save-device-preset", label: "Save Current Parameters as...", icon: "save" } as const] : []),
    { action: "expression", label: "Assign expression pedal", icon: "expression" },
    ...(looper ? [{ action: "assign-looper-actions", label: "Reassign Looper X Actions", icon: "looper" } as const] : []),
    // The unit lists Mute/bypass on every block, not only an FX Loop.
    { action: "mute-bypass", label: "Mute/bypass", icon: "mute" },
    // The unit shows these two on every block regardless of an actual update -
    // it is CorOS's own position for them, not a per-model check this app can
    // run. Always disabled, same honest "not exposed" message as the others.
    { action: "model-update", label: "Model update available", icon: "model-update", disabled: true },
    { action: "legacy-version", label: "Change to legacy version", icon: "model-downgrade", disabled: true },
    { action: "remove", label: "Remove block from the grid", icon: "remove" }
  ];
}

/**
 * A lane control (Input Gate/Lane Output) is not a Grid block: it cannot be
 * saved as a device preset, changed, copied/pasted, reset, assigned Looper X
 * actions, muted/bypassed by this menu, offered a model update, or removed
 * from the Grid - none of `parameterContextMenuItems`'s entries beyond
 * "Assign Expression Pedal" apply to it, so this is the entire menu, not a
 * filtered copy of the other one.
 */
export function laneControlContextMenuItems(): ParameterContextMenuItem[] {
  return [{ action: "expression", label: "Assign expression pedal", icon: "expression" }];
}
