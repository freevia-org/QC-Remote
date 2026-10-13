/** One pairing session owns at most one native poll, including redemption. */
export function createBrowserPairingPoll(options: {
  read(): Promise<{ state: "paired" | "waiting" }>;
  paired(): Promise<void>;
  failed(error: unknown): void;
}) {
  let disposed = false;
  let polling = false;
  return {
    async poll(): Promise<void> {
      if (disposed || polling) return;
      polling = true;
      try {
        const result = await options.read();
        if (!disposed && result.state === "paired") await options.paired();
      } catch (error) {
        if (!disposed) options.failed(error);
      } finally { polling = false; }
    },
    dispose() { disposed = true; }
  };
}
