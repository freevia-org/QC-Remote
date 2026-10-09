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

export function controlAccessPermitsAction(mode: ControlAccessMode, name: string): boolean {
  const action = SHARED_QC_ACTIONS.find((candidate) => candidate.name === name);
  return Boolean(action && accessLevel[mode] >= accessLevel[action.access]);
}
