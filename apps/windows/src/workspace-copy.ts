import type { WorkspaceDocument, WorkspaceFileResult } from "@qc-remote/client";

/** Save the document the Workspace window displays, even while the live QC changes. */
export function saveOpenedWorkspaceCopy(
  writer: { saveAs(document: WorkspaceDocument, suggestedName: string): Promise<WorkspaceFileResult> },
  document: WorkspaceDocument | undefined
): Promise<WorkspaceFileResult> {
  if (!document) return Promise.reject(new Error("No workspace is open. Open a workspace before saving a copy."));
  return writer.saveAs(document, `${document.source.presetLocation} ${document.source.presetName}`);
}
