/** Undefined is a failed inventory read, not evidence that its broker vanished. */
export function createDemoBrokerInstanceObservation(changed: (instance: string | undefined) => void) {
  let observed: string | undefined;
  return (instance: string | null | undefined) => {
    if (instance === undefined) return;
    const next = instance || undefined;
    if (next === observed) return;
    observed = next;
    changed(next);
  };
}

/** Recovery uses the existing inventory cadence, never a new poll or device replay. */
export function createDemoBrokerRecovery(retry: () => void) {
  let needed = false;
  let attempted = false;
  let terminal = false;
  let knownInstance: string | undefined;
  return {
    failed() {
      if (attempted) terminal = true;
      needed = !terminal;
    },
    ready() { needed = false; attempted = false; terminal = false; },
    changed(instance: string | undefined) {
      if (!instance) return false; // Absence is not a confirmed replacement.
      if (knownInstance && knownInstance !== instance) {
        needed = false;
        attempted = false;
        terminal = false;
      }
      knownInstance = instance;
      if (terminal || attempted) return false;
      needed = false;
      attempted = true;
      return true;
    },
    observed(instance: string | null | undefined, starting = false) {
      if (instance) knownInstance ??= instance;
      if (terminal || !needed || !instance || starting) return;
      needed = false;
      attempted = true;
      retry();
    }
  };
}

/** Single admission path for inventory-driven instance changes, including absence. */
export function rebindDemoBrokerFromObservation(
  recovery: { changed(instance: string | undefined): boolean },
  instance: string | undefined,
  starting: boolean,
  rebind: (available: boolean) => void
) {
  if (starting) return;
  if (!instance) rebind(false);
  else if (recovery.changed(instance)) rebind(true);
}

/** Native worker creation is not proof that the broker accepted its provider. */
export async function waitForDemoBrokerRegistration(options: {
  start(): Promise<void>;
  subscribe(observe: (ready: boolean, message?: string) => void): () => void;
  timeoutMs?: number;
}): Promise<void> {
  let stop: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let failure: Error | undefined;
  const ready = new Promise<void>((resolve, reject) => {
    const refuse = (message: string) => {
      failure ??= new Error(message);
      reject(failure);
    };
    timer = setTimeout(() => refuse("The broker did not acknowledge Demo registration"), options.timeoutMs ?? 10_000);
    stop = options.subscribe((ready, message) => {
      if (!ready) refuse(message || "The Demo provider is unavailable");
      else if (!failure) resolve();
    });
  });
  // Observe an early failure while native startup itself is still pending.
  void ready.catch(() => undefined);
  try {
    await options.start();
    await ready;
    // A positive event can precede native command completion. Loss/cancel
    // after it is terminal, even though the ready Promise already resolved.
    if (failure) throw failure;
  }
  finally { if (timer !== undefined) clearTimeout(timer); stop?.(); }
}

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
