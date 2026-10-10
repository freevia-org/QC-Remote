/** What leaves the device host on its way to a model.
 *
 * Every result returned to a remote assistant passes through an identifier
 * strip: the fields in `contracts/public/qc-relay-profile.v1.json`
 * `removedIdentifierFields` are removed, and a backup payload is replaced by a
 * note. The list is read from the shared contract rather than copied here, so
 * it cannot drift from what the native hosts publish. */
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

export const REMOVED_IDENTIFIER_FIELDS: readonly string[] = relayProfile.removedIdentifierFields;

/** A full device backup is every preset, capture, IR and setting on the unit,
 * base64 encoded. Returned over the relay it becomes a copy of the user's
 * entire library inside a third-party model's context window, which is the one
 * thing the assistant guide's privacy section tells the model not to do. The
 * host keeps the bytes; the model is told what exists and how to get it. */
const BACKUP_PAYLOAD_NOTE =
  "The backup was created on the device host. Its contents are deliberately not sent to a remote assistant; open QC Remote on the paired computer or phone to save the file.";

function isBackupDocument(value: Record<string, unknown>): boolean {
  return value.type === "backup" && typeof value.payload === "string";
}

/** Remove persistent hardware identifiers, and replace a backup's payload with
 * a description of it. Structure is otherwise preserved exactly. */
export function sanitizePublicResult<T>(value: T): T {
  const visit = (node: unknown): unknown => {
    if (Array.isArray(node)) return node.map(visit);
    if (node === null || typeof node !== "object") return node;
    const source = node as Record<string, unknown>;
    const output: Record<string, unknown> = {};
    const backup = isBackupDocument(source);
    for (const [key, child] of Object.entries(source)) {
      if (REMOVED_IDENTIFIER_FIELDS.includes(key.toLowerCase())) continue;
      if (backup && key === "payload") {
        output.payloadBytes = (child as string).length;
        output.payloadWithheld = BACKUP_PAYLOAD_NOTE;
        continue;
      }
      output[key] = visit(child);
    }
    return output;
  };
  return visit(value) as T;
}
