type AttachedHost = { deserializeAttachment(): unknown; readyState?: number };

const OFFLINE_HOST_METHODS = new Set([
  "system.status",
  "device.reconnect",
  "device.resetSession",
  "device.disconnect"
]);

/** Nothing is paired to this account at all.
 *
 * Kept distinct from the USB message below because the two have completely
 * different causes and only one of them is about USB. A host reaches its owner
 * through the device directory written when it paired; a model reaches the same
 * owner through the Google subject in its OAuth token. Pair with one Google
 * account and authorise the connector with another and both halves work
 * perfectly while never meeting — the device heartbeats every second into a
 * namespace the assistant never reads. Saying "no USB-ready device" there sends
 * someone to check a cable that was never the problem. */
export const NO_PAIRED_HOST =
  "No paired device is connected to this account. If QC Remote shows it as paired, check that the Google account signed in there is the one that authorised this connector.";

/** USB readiness must not prevent the one connected host from reopening USB. */
export function selectDeviceHost<T extends AttachedHost>(sockets: readonly T[], method: string): T {
  const hosts = sockets.filter((socket) => {
    if (socket.readyState !== undefined && socket.readyState !== 1) return false;
    const value = socket.deserializeAttachment() as { deviceId?: unknown } | null;
    return typeof value?.deviceId === "string" && value.deviceId.length > 0;
  });
  const ready = hosts.filter((socket) =>
    (socket.deserializeAttachment() as { ready?: unknown }).ready === true);
  if (ready.length > 1) throw new Error("Multiple USB-ready paired hosts are connected; the target is ambiguous");
  if (ready.length === 1) return ready[0];
  if (OFFLINE_HOST_METHODS.has(method)) {
    if (hosts.length === 1) return hosts[0];
    if (hosts.length > 1) throw new Error("Multiple paired hosts are connected without USB readiness; the target is ambiguous");
  }
  if (hosts.length === 0) throw new Error(NO_PAIRED_HOST);
  throw new Error("No USB-ready paired device is connected");
}
