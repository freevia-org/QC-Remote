/** Serialize begin/cancel so closing during a slow begin withdraws its late offer. */
export function createBrowserPairingSession<Offer>(port: { begin(): Promise<Offer>; cancel(): Promise<void> }) {
  let generation = 0;
  let tail: Promise<unknown> = Promise.resolve();
  const enqueue = <T,>(run: () => Promise<T>): Promise<T> => {
    const next = tail.then(run, run);
    tail = next.catch(() => undefined);
    return next;
  };
  return {
    checkpoint() { const origin = generation; return () => origin === generation; },
    async begin() {
      const origin = generation;
      const offer = await enqueue(() => port.begin());
      return { offer, current: () => origin === generation };
    },
    cancel() {
      generation += 1;
      return enqueue(() => port.cancel());
    }
  };
}

/** A slow network poll owns its interval; another poll cannot overtake it. */
export function startSequentialPairingPoll(
  poll: (current: () => boolean) => Promise<void>,
  schedule: (run: () => void) => unknown = (run) => setTimeout(run, 2_000),
  cancel: (handle: unknown) => void = (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
) {
  let active = true;
  let timer: unknown;
  const tick = async () => {
    if (!active) return;
    try { await poll(() => active); }
    finally { if (active) timer = schedule(() => { void tick(); }); }
  };
  void tick();
  return () => { active = false; if (timer !== undefined) cancel(timer); };
}
