/** Own lifecycle registration only. This controller never retries device work. */
export function createDemoBrokerLifecycle(options: {
  newId(): string;
  start(id: string): Promise<void>;
  stop(id: string): Promise<void>;
  invalidate(): void;
  failed(error: unknown): void;
}) {
  let id = options.newId();
  let active: string | undefined;
  let live = true;
  let queue = Promise.resolve();
  const stopActive = async () => {
    const previous = active;
    active = undefined;
    if (previous) await options.stop(previous);
  };
  const start = async (captured: string) => {
    if (!live || captured !== id) return;
    try { await options.start(captured); }
    catch (error) {
      // An IPC failure can arrive after native startup ran. Retire that exact
      // attempted id as well; never leave an untracked registration behind.
      await options.stop(captured).catch(() => undefined);
      await stopActive();
      throw error;
    }
    active = captured;
    if (!live || captured !== id) await stopActive();
  };
  const enqueue = (operation: () => Promise<void>) => {
    const result = queue.then(operation);
    queue = result.catch(error => { if (live) options.failed(error); });
    return result;
  };
  return {
    get id() { return id; },
    isCurrent(captured: string) { return live && id === captured; },
    start() { const captured = id; return enqueue(() => start(captured)); },
    rebind(available: boolean) {
      id = options.newId();
      const captured = id;
      options.invalidate();
      // Each native worker has a different id, so late replies/status from
      // the previous worker cannot invalidate the replacement registration.
      void enqueue(() => available ? start(captured) : stopActive()).catch(() => undefined);
    },
    dispose() {
      if (!live) return queue;
      live = false;
      options.invalidate();
      const stopped = enqueue(stopActive);
      void stopped.catch(() => undefined);
      return stopped;
    }
  };
}
