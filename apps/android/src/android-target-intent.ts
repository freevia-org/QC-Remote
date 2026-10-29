export type AndroidTargetSource = "demo" | "physical";
export type AndroidTargetAcknowledgement = { source: AndroidTargetSource; revision: number };

/** Selection is user intent. Provider readiness and USB discovery cannot change it. */
export function createAndroidTargetIntent(changed: (source: AndroidTargetSource, acknowledged: boolean) => void) {
  let source: AndroidTargetSource = "physical", epoch = 0, pending = false, verified = false;
  const valid = (reply: AndroidTargetAcknowledgement) => reply && (reply.source === "demo" || reply.source === "physical")
    && Number.isSafeInteger(reply.revision) && reply.revision >= 0;
  return {
    get source() { return source; },
    get acknowledged() { return verified; },
    get canAutomaticallyConnectPhysical() { return verified && source === "physical" && !pending; },
    async restore(read: () => Promise<AndroidTargetAcknowledgement>) {
      const owner = epoch;
      const reply = await read();
      if (!valid(reply)) throw new Error("The selected device could not be verified.");
      if (owner !== epoch) return;
      source = reply.source; verified = true; changed(source, true);
    },
    async select(next: AndroidTargetSource, acknowledge: () => Promise<AndroidTargetAcknowledgement>) {
      const owner = ++epoch;
      pending = true;
      // Block physical adoption immediately when Demo is requested, including
      // a failed/ambiguous native acknowledgement or provider registration.
      if (next === "demo") { source = next; verified = false; changed(source, false); }
      try {
        const reply = await acknowledge();
        if (!valid(reply) || reply.source !== next) throw new Error("The device selection was not acknowledged.");
        if (owner !== epoch) return false;
        source = next; verified = true; changed(source, true);
        return true;
      } finally { if (owner === epoch) pending = false; }
    }
  };
}
