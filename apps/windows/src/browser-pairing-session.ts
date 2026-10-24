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
      // Invalidate every earlier offer/poll immediately, before this begin
      // queues behind native work. The fence survives begin completion.
      const origin = ++generation;
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
  poll: (current: () => boolean) => Promise<void | boolean>,
  schedule: (run: () => void) => unknown = (run) => setTimeout(run, 2_000),
  cancel: (handle: unknown) => void = (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
) {
  let active = true;
  let timer: unknown;
  const tick = async () => {
    if (!active) return;
    try { if (await poll(() => active) === false) active = false; }
    finally { if (active) timer = schedule(() => { void tick(); }); }
  };
  void tick();
  return () => { active = false; if (timer !== undefined) cancel(timer); };
}

/** Opening a paired/unknown status panel is not consent to create another offer. */
export function beginBrowserPairingOnPanelOpen(paired: boolean | undefined, hasOffer: boolean, starting: boolean, begin: () => void) {
  if (paired === false && !hasOffer && !starting) begin();
}

type PairingClock = {
  now(): number;
  schedule(run: () => void, delay: number): unknown;
  cancel(handle: unknown): void;
};

/** One offer owns both timers. Success, expiry, error, and close are terminal. */
export function startBrowserPairingLifecycle(
  flow: {
    expiresAt: number;
    isCurrent?(): boolean;
    poll(): Promise<{ state: "waiting" | "paired" }>;
    onPaired(): void;
    onExpired(): void;
    onFailure(error: unknown): void;
  },
  clock: PairingClock = {
    now: Date.now,
    schedule: (run, delay) => setTimeout(run, delay),
    cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
  }
) {
  let active = true;
  let stopPoll: (() => void) | undefined;
  let stopExpiry: (() => void) | undefined;
  const current = () => active && (flow.isCurrent?.() ?? true);
  const finish = (apply?: () => void) => {
    if (!active) return;
    const applyAllowed = current();
    active = false;
    stopPoll?.();
    stopExpiry?.();
    if (applyAllowed) apply?.();
  };
  stopExpiry = startBrowserPairingExpiry(flow.expiresAt, () => finish(flow.onExpired), clock.now, clock.schedule, clock.cancel);
  stopPoll = startSequentialPairingPoll(async (pollCurrent) => {
    if (!current() || !pollCurrent()) {
      finish();
      return false;
    }
    if (clock.now() >= flow.expiresAt) {
      finish(flow.onExpired);
      return false;
    }
    try {
      const result = await flow.poll();
      if (!current() || !pollCurrent()) {
        finish();
        return false;
      }
      if (result.state === "paired") {
        finish(flow.onPaired);
        return false;
      }
    } catch (error) {
      finish(() => flow.onFailure(error));
      return false;
    }
  }, (run) => clock.schedule(run, 2_000), clock.cancel);
  return () => finish();
}

export const BROWSER_PAIRING_EXPIRED = "Pairing code expired. Choose Try again to create a new code.";

/** Expiry ends this offer; only a new explicit action may create its replacement. */
export function startBrowserPairingExpiry(
  expiresAt: number,
  expire: () => void,
  now: () => number = Date.now,
  schedule: (run: () => void, delay: number) => unknown = (run, delay) => setTimeout(run, delay),
  cancel: (handle: unknown) => void = (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>)
) {
  let active = true;
  const timer = schedule(() => {
    if (!active) return;
    active = false;
    expire();
  }, Math.max(0, expiresAt - now()));
  return () => { active = false; cancel(timer); };
}

/** A failed poll must not leave the panel looking as if it is still creating a code.
 * Keep backend details out of the pairing surface; the user can explicitly retry.
 */
export function browserPairingPollFailureMessage(error: unknown): string {
  if (error instanceof Error && error.message === "The browser pairing session expired. Start it again from QC Remote.") {
    return error.message;
  }
  return "Could not check the pairing session. Retry to create a new code.";
}
