export type ConnectorUnlinkOutcome = "unlinked" | "cancelled" | "superseded" | "failed";

/**
 * One Unlink of a linked Remote MCP app. A failed unlink is an action failure,
 * reported through `onFailed`: it must never be written into the list's own
 * read-error slot, which hides every linked app behind "Could not read".
 */
export async function runConnectorUnlink<List>(operation: {
  confirm(): Promise<boolean> | boolean;
  unlink(): Promise<List>;
  /** False once the window, the mount or this mutation has moved on. */
  current(): boolean;
  onUnlinked(connectors: List): void;
  onFailed(error: unknown): void;
}): Promise<ConnectorUnlinkOutcome> {
  try {
    if (!await operation.confirm() || !operation.current()) return "cancelled";
    const connectors = await operation.unlink();
    if (!operation.current()) return "superseded";
    operation.onUnlinked(connectors);
    return "unlinked";
  } catch (error) {
    if (!operation.current()) return "superseded";
    operation.onFailed(error);
    return "failed";
  }
}

/** The list is re-read after a failure, because whether the unlink took effect is not known. */
export function connectorsNeedRefreshAfter(outcome: ConnectorUnlinkOutcome): boolean {
  return outcome === "failed";
}
