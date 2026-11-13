import { SHARED_QC_ACCESS_MODES, SHARED_QC_ACTIONS, type SharedQcAccessMode } from "./generated-actions.ts";

export type ControlAccessMode = SharedQcAccessMode;

const accessLevel = Object.fromEntries(
  SHARED_QC_ACCESS_MODES.map((mode, index) => [mode, index])
) as Record<ControlAccessMode, number>;

export function parseControlAccessMode(value: unknown, fallback: ControlAccessMode = "full"): ControlAccessMode {
  return typeof value === "string" && (SHARED_QC_ACCESS_MODES as readonly string[]).includes(value)
    ? value as ControlAccessMode
    : fallback;
}

const ACTION_ACCESS_BY_NAME = new Map<string, SharedQcAccessMode>(SHARED_QC_ACTIONS.map((action) => [action.name, action.access] as const));

export function controlAccessPermitsAction(mode: ControlAccessMode, name: string): boolean {
  const access = ACTION_ACCESS_BY_NAME.get(name);
  return access !== undefined && accessLevel[mode] >= accessLevel[access];
}
