import type { QcCommandResult } from "./transport.ts";
import { sceneLetter } from "./state.ts";

export type QcDeviceCommand =
  | { kind: "scene"; scene: number }
  | { kind: "preset-step"; delta: -1 | 1 }
  | { kind: "tempo"; bpm: number }
  | { kind: "bypass"; blockId: string; row: number; column: number; bypassed: boolean; blockName: string }
  | { kind: "view"; view: "tuner" | "gig"; show: boolean };

export function deviceCommandDetail(command: QcDeviceCommand, result: QcCommandResult): string {
  if (result.detail) return result.detail;
  if (command.kind === "scene") return `Scene ${sceneLetter(command.scene)} selected.`;
  if (command.kind === "preset-step") return `Moved to the ${command.delta > 0 ? "next" : "previous"} preset.`;
  if (command.kind === "tempo") return `Tempo set to ${command.bpm} BPM.`;
  if (command.kind === "bypass") return `${command.blockName} ${command.bypassed ? "bypassed" : "enabled"}.`;
  return `${command.view === "tuner" ? "Tuner" : "Gig View"} ${command.show ? "opened" : "closed"}.`;
}
