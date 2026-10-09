import { parseControlAccessMode, type ControlAccessMode } from "@qc-remote/core";

export const CONTROL_ACCESS_MODE_STORAGE_KEY = "qc.control.access-mode.v2";
const LEGACY_ACCESS_MODE_STORAGE_KEY = "qc.control.assistant-access-mode.v1";

type AccessModeStorage = Pick<Storage, "getItem" | "setItem">;

export function readControlAccessMode(storage: AccessModeStorage, legacyKeys: readonly string[] = []): ControlAccessMode {
  const current = storage.getItem(CONTROL_ACCESS_MODE_STORAGE_KEY);
  if (current !== null) return parseControlAccessMode(current);
  for (const key of [LEGACY_ACCESS_MODE_STORAGE_KEY, ...legacyKeys]) {
    const legacy = storage.getItem(key);
    if (legacy === null) continue;
    const mode = parseControlAccessMode(legacy);
    storage.setItem(CONTROL_ACCESS_MODE_STORAGE_KEY, mode);
    return mode;
  }
  return parseControlAccessMode(undefined);
}

export function writeControlAccessMode(storage: AccessModeStorage, mode: ControlAccessMode): void {
  storage.setItem(CONTROL_ACCESS_MODE_STORAGE_KEY, mode);
}
