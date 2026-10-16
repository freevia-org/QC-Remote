import type { BlockDetails, PresetSnapshot, WorkspaceDocument, WorkspaceFileResult } from "@qc-remote/client";
import { withoutModelPresetCatalog } from "./model-preset-catalog.ts";

/**
 * The document a Windows workspace file stores. Its snapshot never carries the device's model
 * preset list: that list belongs to one device session and is not user data to keep on disk.
 * The Windows app saves through saveWorkspaceDocument below, which builds its document here, and
 * behaviour tests run that path with a recording writer. That is a convention, not a boundary: the
 * Rust command validates the stable version-1 document fields before reading or writing but does
 * not remove the catalog, so a caller that builds its own document would bypass this builder's
 * catalog exclusion.
 */
export function buildWorkspaceDocument(input: {
  snapshot: PresetSnapshot;
  selectedBlock?: BlockDetails;
  selectedBlockId: string;
  formFactorId: string;
  savedAt?: string;
}): WorkspaceDocument {
  const { snapshot } = input;
  return {
    version: 1,
    savedAt: input.savedAt ?? new Date().toISOString(),
    source: {
      deviceName: snapshot.deviceName,
      setlistKey: snapshot.setlistKey,
      setlistName: snapshot.setlistName,
      presetPosition: snapshot.presetPosition,
      presetLocation: snapshot.presetLocation,
      presetName: snapshot.presetName
    },
    snapshot: withoutModelPresetCatalog(snapshot) as PresetSnapshot,
    selectedBlock: input.selectedBlock,
    ui: { selectedBlockId: input.selectedBlockId, formFactorId: input.formFactorId }
  };
}

/** The two writes the Windows app can make; the app passes its `workspaceFiles`. */
export interface WorkspaceWriter {
  save(path: string, document: WorkspaceDocument): Promise<WorkspaceFileResult>;
  saveAs(document: WorkspaceDocument, suggestedName: string): Promise<WorkspaceFileResult>;
}

/**
 * Build the document and hand it to the writer: `save` when there is a path and the caller did not
 * ask for a copy, `saveAs` otherwise. The document returned is the one that was written, so a
 * caller keeps the stripped copy too. This is the path a test can run end to end with a writer
 * that records what it was given.
 */
export async function saveWorkspaceDocument(
  writer: WorkspaceWriter,
  input: Parameters<typeof buildWorkspaceDocument>[0] & { path?: string; saveAs: boolean; suggestedName: string }
): Promise<{ document: WorkspaceDocument; result: WorkspaceFileResult }> {
  const document = buildWorkspaceDocument(input);
  const result = !input.saveAs && input.path
    ? await writer.save(input.path, document)
    : await writer.saveAs(document, input.suggestedName);
  return { document, result };
}
