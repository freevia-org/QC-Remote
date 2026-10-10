import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

/** Host results have their own protocol bound; a catalog exceeds request size. */
export function decodeDeviceMessage(message: string | ArrayBuffer): Record<string, unknown> {
  const limit = relayProfile.maxResultFrameBytes;
  if (typeof message !== "string" || message.length > limit ||
      new TextEncoder().encode(message).byteLength > limit) {
    throw new Error("Invalid message");
  }
  const parsed: unknown = JSON.parse(message);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("Invalid message");
  }
  return parsed as Record<string, unknown>;
}
