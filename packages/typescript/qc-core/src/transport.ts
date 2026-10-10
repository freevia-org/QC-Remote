import type { BlockDetails, PresetSnapshot } from "@qc-remote/client";

export interface QcCommandResult {
  detail?: string;
  snapshot?: PresetSnapshot;
  block?: BlockDetails;
}

export interface QcDeviceTransport {
  currentSnapshot(): Promise<PresetSnapshot>;
  selectScene(scene: number, expected?: PresetSnapshot): Promise<QcCommandResult>;
  selectModeSlot(slot: 0 | 1 | 2, expected?: PresetSnapshot): Promise<QcCommandResult>;
  movePreset(delta: -1 | 1, expected?: PresetSnapshot): Promise<QcCommandResult>;
  pressFootswitch(index: number, expected?: PresetSnapshot): Promise<QcCommandResult>;
  setTempo(bpm: number, expected?: PresetSnapshot): Promise<QcCommandResult>;
  tapTempo(expected?: PresetSnapshot): Promise<QcCommandResult>;
  blockDetails(row: number, column: number, expected?: PresetSnapshot): Promise<BlockDetails>;
  setParameter(row: number, column: number, parameterIndex: number, value: number, expected?: PresetSnapshot): Promise<QcCommandResult>;
  setBypass(row: number, column: number, bypassed: boolean, expected?: PresetSnapshot): Promise<QcCommandResult>;
  setTuner(show: boolean, expected?: PresetSnapshot): Promise<QcCommandResult>;
  setGigView(show: boolean, expected?: PresetSnapshot): Promise<QcCommandResult>;
}
