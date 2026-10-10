import { useCallback, useEffect, useRef, useState } from "react";
import type { ControlAccessMode, PublicRelayPort, PublicRelayState, PublicRelayStatus } from "@qc-remote/core";

export interface PublicRelaySubscription {
  remove(): void | Promise<void>;
}

export interface PublicRelayWorkflowOptions {
  relay: PublicRelayPort;
  enabled: boolean;
  pollIntervalMs?: number;
  autoStart?: boolean;
  subscribe?: (listener: (state: PublicRelayState) => void) => Promise<PublicRelaySubscription>;
}

/** Shared pairing and lifecycle controller; native hosts only adapt calls and events. */
export function usePublicRelayWorkflow(options: PublicRelayWorkflowOptions) {
  const { relay, enabled, pollIntervalMs = 0, autoStart = false, subscribe } = options;
  const [status, setStatus] = useState<PublicRelayStatus>();
  const [pending, setPending] = useState(false);

  const lifecycle = useRef(0);
  const active = useRef(false);
  const mutationRevision = useRef(0);
  const readRevision = useRef(0);
  const autoStartSuppressed = useRef(false);
  const pendingCount = useRef(0);
  const mutationQueue = useRef<Promise<void>>(Promise.resolve());

  const run = useCallback(async (
    operation: (current: () => boolean) => Promise<PublicRelayStatus>,
    onlyIfLatest = false
  ) => {
    if (!enabled || !active.current) throw new Error("Relay controls are unavailable");
    const epoch = lifecycle.current;
    const revision = ++mutationRevision.current;
    const current = () => active.current && lifecycle.current === epoch;
    pendingCount.current++;
    setPending(true);
    // Pair/start/unpair all mutate one native relay. Ordering their results is
    // insufficient: an older pair must finish before a newer unpair is sent.
    const work = mutationQueue.current.then(async () => {
      if (!current() || (onlyIfLatest && revision !== mutationRevision.current)) {
        throw new Error("Relay operation was superseded");
      }
      const next = await operation(current);
      if (current() && revision === mutationRevision.current) setStatus(next);
      return next;
    });
    mutationQueue.current = work.then(() => undefined, () => undefined);
    try {
      return await work;
    } finally {
      if (current()) {
        pendingCount.current--;
        setPending(pendingCount.current > 0);
      }
    }
  }, [enabled, relay]);

  const refresh = useCallback(() => run(() => relay.status()), [relay, run]);
  const pair = useCallback((endpoint: string, pairingCode: string, deviceName?: string) =>
    run(() => relay.pair(endpoint, pairingCode, deviceName)), [relay, run]);
  const startOperation = useCallback(async (current: () => boolean) => {
    await relay.start();
    if (!current()) throw new Error("Relay operation was superseded");
    return relay.status();
  }, [relay]);
  const start = useCallback(() => run(startOperation), [run, startOperation]);
  const unpair = useCallback(() => {
    autoStartSuppressed.current = true;
    return run(() => relay.unpair());
  }, [relay, run]);
  const setAccessMode = useCallback((mode: ControlAccessMode) =>
    run(() => relay.setAccessMode(mode)), [relay, run]);

  useEffect(() => {
    lifecycle.current++;
    active.current = enabled;
    autoStartSuppressed.current = false;
    mutationRevision.current++;
    readRevision.current++;
    pendingCount.current = 0;
    setPending(false);
    setStatus(undefined);
    return () => {
      active.current = false;
      lifecycle.current++;
    };
  }, [enabled, relay]);

  useEffect(() => {
    if (!enabled) return;
    const epoch = lifecycle.current;
    let disposed = false;
    let subscription: PublicRelaySubscription | undefined;
    let reading = false;
    let bootstrapPending = true;
    const current = () => !disposed && active.current && lifecycle.current === epoch;
    const observeStatus = async () => {
      // Poll/focus events cannot accumulate an unbounded queue behind a slow
      // status call or publish intermediate state while a mutation is pending.
      if (!current() || reading || pendingCount.current > 0) return;
      reading = true;
      const mutation = mutationRevision.current;
      const read = ++readRevision.current;
      try {
        const next = await relay.status();
        if (!current() || mutation !== mutationRevision.current || read !== readRevision.current) return;
        setStatus(next);
        const allowStart = bootstrapPending && autoStart && !autoStartSuppressed.current;
        bootstrapPending = false;
        if (allowStart && next.paired && next.state === "stopped") {
          await run((operationCurrent) => {
            if (!current() || autoStartSuppressed.current) throw new Error("Relay operation was superseded");
            return startOperation(operationCurrent);
          }, true);
        }
      } finally {
        reading = false;
      }
    };
    const remove = (value: PublicRelaySubscription) => {
      void Promise.resolve().then(() => value.remove()).catch(() => undefined);
    };
    void observeStatus().catch(() => undefined);
    if (subscribe) {
      void subscribe((state) => {
        if (current() && pendingCount.current === 0) {
          readRevision.current++;
          setStatus((previous) => previous ? { ...previous, state } : previous);
        }
      }).then((value) => {
        if (current()) subscription = value;
        else remove(value);
      }).catch(() => undefined);
    }
    const timer = pollIntervalMs > 0
      ? window.setInterval(() => void observeStatus().catch(() => undefined), pollIntervalMs)
      : undefined;
    const refreshOnReturn = () => {
      if (document.visibilityState === "visible") void observeStatus().catch(() => undefined);
    };
    window.addEventListener("focus", refreshOnReturn);
    document.addEventListener("visibilitychange", refreshOnReturn);
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearInterval(timer);
      window.removeEventListener("focus", refreshOnReturn);
      document.removeEventListener("visibilitychange", refreshOnReturn);
      if (subscription) remove(subscription);
    };
  }, [autoStart, enabled, pollIntervalMs, relay, subscribe, run, startOperation]);

  return { status, pending, refresh, pair, start, unpair, setAccessMode };
}
