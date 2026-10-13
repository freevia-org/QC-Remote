import type { GridBlock, PresetSnapshot } from "@qc-remote/client";
import type { CorOsScreenView } from "./coros-screen-fixture-data.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";

/** The device screens the hosts draw in place of the Grid. */
export type QcDeviceScreenKind = "device-settings" | "tempo" | "tuner" | "looper-editor" | "modes" | "midi-out" | "global-eq" | "io" | "unit-mirror";

/**
 * Which device screen to draw. Device Settings and Tempo are workflows with
 * their own open flag and win over the navigated view, in that order; every
 * other screen is chosen by the view. One precedence for both hosts.
 */
export function resolveQcDeviceScreen(view: CorOsScreenView | null, open: { deviceSettings: boolean; tempo: boolean }): QcDeviceScreenKind | undefined {
  if (open.deviceSettings) return "device-settings";
  if (open.tempo) return "tempo";
  if (view === "tuner" || view === "looper-editor" || view === "modes" || view === "midi-out" || view === "global-eq" || view === "unit-mirror") return view;
  if (view?.startsWith("io-")) return "io";
  return undefined;
}

/** The Looper X block the Looper screen's Parameters and Menu buttons open. */
export function findLooperXBlock(snapshot: Pick<PresetSnapshot, "blocks">): GridBlock | undefined {
  return snapshot.blocks.find((block) => block.modelId === 27_000 || block.name === "Looper X");
}

export interface QcLooperScreenOptions {
  captureSessionCurrent?(): () => boolean;
  snapshot: Pick<PresetSnapshot, "blocks">;
  looper: { run(command: string, value?: number | null): Promise<void | boolean> };
  grid: { openBlock(block: GridBlock): Promise<unknown> };
  setView: QcScreenDestination["setView"];
  /** Host flag that opens the block editor with its parameter menu showing. */
  setParameterMenuInitiallyOpen(open: boolean): void;
  notice(message: string): void;
}

/**
 * Leave the Looper X screen: tell the unit first, and only then clear the
 * view, so the app never shows the Grid while the unit is still in Looper.
 */
export async function closeQcLooperScreen({ looper, setView, captureSessionCurrent }: Pick<QcLooperScreenOptions, "looper" | "setView" | "captureSessionCurrent">): Promise<void> {
  const current = captureSessionCurrent?.() ?? (() => true);
  if (!current()) return;
  if (await looper.run("close") === false || !current()) return;
  setView(null);
}

/** What the Looper X screen's Parameters, Menu and Close buttons do. */
export function qcLooperScreenActions(options: QcLooperScreenOptions) {
  const { snapshot, looper, grid, setView, setParameterMenuInitiallyOpen, notice } = options;
  const current = options.captureSessionCurrent?.() ?? (() => true);
  const openEditor = async (what: "parameters" | "menu") => {
    if (!current()) return;
    const block = findLooperXBlock(snapshot);
    if (!block) { notice(`Add a Looper X block to this preset before opening its ${what}.`); return; }
    if (await looper.run("performMode", 1) === false || !current()) return;
    if (what === "menu") setParameterMenuInitiallyOpen(true);
    setView(null);
    await grid.openBlock(block);
  };
  return {
    onParameters: () => openEditor("parameters"),
    onMenu: () => openEditor("menu"),
    onClose: () => closeQcLooperScreen({ looper, setView, captureSessionCurrent: () => current })
  };
}
