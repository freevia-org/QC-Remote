import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { GatewayTransport, PresetEntry, PresetList, PresetSnapshot } from "@qc-remote/client";
import type { PresetLifetime, PresetOperation } from "@qc-remote/core";
import type { PresetDirectoryState } from "./directory-library-model.ts";
import type { WorkflowPrompts } from "./workflow-options";

export type PresetClipboard = Pick<PresetSnapshot,
  "setlistKey" | "setlistName" | "presetPosition" | "presetLocation" | "presetName"
> & { mode?: "copy" | "cut" };

export interface UsePresetWorkflowOptions {
  lifetime?: PresetLifetime;
  gateway: GatewayTransport;
  snapshotRef: MutableRefObject<PresetSnapshot>;
  setSnapshot: Dispatch<SetStateAction<PresetSnapshot>>;
  acceptSnapshot?(snapshot: PresetSnapshot): PresetSnapshot;
  connected: boolean;
  pending: boolean;
  setPending: Dispatch<SetStateAction<boolean>>;
  prompts: WorkflowPrompts;
  notice(message: string): void;
  fail(error: unknown): void;
  onPresetChanged?(snapshot: PresetSnapshot): void;
}

export interface PresetWorkflowController {
  presetList?: PresetList;
  directoryOpen: boolean;
  directoryLoading: boolean;
  /**
   * A directory pull the host is still waiting on, including the background
   * warm-up that runs on device readiness before the browser is ever opened.
   * The broker holds a catalog read until this USB session's own directory
   * publication has reached the requested setlist, so an outstanding call is
   * exactly "the pull has not arrived yet" and the host can say so without
   * presenting a partial directory as the whole truth.
   */
  catalogPending: boolean;
  saveOpen: boolean;
  saveName: string;
  clipboard?: PresetClipboard;
  /**
   * Open the preset browser. `reload` re-reads this session's published
   * catalog; it is never the gateway's `refresh`, which the QC cannot serve.
   */
  openDirectory(reload?: boolean): Promise<void>;
  closeDirectory(): void;
  /** Keeps the cloud-linked state the unit's own Directory indicator shows (see directory-cloud-link.ts). */
  followCloudSignedIn(signedIn: boolean): void;
  loadDirectory(refresh?: boolean, setlistKey?: string, quiet?: boolean): Promise<PresetList | undefined>;
  recall(entry: PresetEntry): Promise<void>;
  recallLocation(location: string): Promise<string>;
  reload(): Promise<void>;
  openSave(): void;
  closeSave(): void;
  setSaveName(name: string): void;
  save(): Promise<void>;
  saveCurrentUnsaved(name: string): Promise<string>;
  copy(): void;
  paste(): Promise<void>;
  rename(suggestedName?: string): Promise<void>;
  createSetlist(): Promise<void>;
  refresh(): Promise<void>;
  resetCache(): void;
  clearClipboard(): void;
  directoryProps: PresetDirectoryState;
  saveProps: {
    open: boolean;
    name: string;
    disabled: boolean;
    onNameChange(name: string): void;
    onSave(): void;
    onCancel(): void;
  };
}
