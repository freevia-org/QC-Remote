import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

/** Host results have their own protocol bound; a catalog exceeds request size. */
export function decodeDeviceMessage(message: string | ArrayBuffer): Record<string, unknown> {
  const limit = relayProfile.maxResultFrameBytes;
  // A UTF-16 unit is at most 3 UTF-8 bytes, so a message whose length is at or
  // below a third of the limit cannot exceed it and is not encoded just to be
  // measured. Accepts and rejects exactly the same messages as encoding all.
  if (typeof message !== "string" || message.length > limit ||
      (message.length * 3 > limit && new TextEncoder().encode(message).byteLength > limit)) {
    throw new Error("Invalid message");
  }
  const parsed: unknown = JSON.parse(message);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("Invalid message");
  }
  return parsed as Record<string, unknown>;
}
