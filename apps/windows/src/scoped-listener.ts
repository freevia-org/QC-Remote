type Listen = <T>(event: string, handler: (event: { payload: T }) => void) => Promise<() => void>;

/**
 * Attach a native listener for the life of an owner. Returns the owner's
 * dispose function. Listening is asynchronous, so a dispose that comes before
 * the attachment resolves stops the late listener the moment it arrives, and a
 * dispose after it stops it once.
 */
export function listenScoped<T>(
  listen: Listen,
  event: string,
  handler: (event: { payload: T }) => void,
  onError: (error: unknown) => void = (error) => { console.error(`QC Remote could not listen for ${event}:`, error); }
): () => void {
  let disposed = false;
  let unlisten: (() => void) | undefined;
  void listen<T>(event, (received) => { if (!disposed) handler(received); })
    .then((stop) => { if (disposed) stop(); else unlisten = stop; })
    .catch((error: unknown) => { if (!disposed) onError(error); });
  return () => {
    if (disposed) return;
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  };
}
