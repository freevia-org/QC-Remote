import { bindDeviceCommandScope, claimDeviceCommand, completeDeviceCommand, deviceCommandBusy, registerDeviceCommandSession } from "./device-command-ownership.ts";
import { DeviceMutationAdmission, type DeviceMutationLease, type PresetLifetime } from "@qc-remote/core";
import { useCallback, useEffect, useRef, useState } from "react";

/** One control's write slot: at most one write in flight, newest target wins. */
export type LatestValueQueue<T = number> = {
  running: boolean;
  target?: T;
};

type TargetOf<Q extends LatestValueQueue<unknown>> = NonNullable<Q["target"]>;

/**
 * Write the newest target of `queue` until none is left, one write at a time.
 *
 * A target that arrives while a write is in flight replaces any older waiting
 * target instead of being dropped, so the last value of a gesture is always
 * the last value written. `guard` supplies the precondition for each write
 * (for tempo, the value the device last confirmed); returning `undefined`
 * stops the drain without writing.
 */
export async function drainLatestValue<Q extends LatestValueQueue<unknown>, R>(queue: Q, handlers: {
  guard(queue: Q): number | undefined;
  write(target: TargetOf<Q>, expected: number, queue: Q): Promise<R>;
  accept(result: R, target: TargetOf<Q>, queue: Q): void;
  drained?(queue: Q): void;
  error(error: unknown, queue: Q): void | Promise<void>;
  idle?(queue: Q): void;
  reschedule(): void;
}) {
  if (queue.running || queue.target === undefined || handlers.guard(queue) === undefined) return;
  queue.running = true;
  try {
    while (queue.target !== undefined) {
      const target = queue.target as TargetOf<Q>;
      const expected = handlers.guard(queue);
      if (expected === undefined) return;
      queue.target = undefined;
      const result = await handlers.write(target, expected, queue);
      handlers.accept(result, target, queue);
    }
    handlers.drained?.(queue);
  } catch (error) {
    await handlers.error(error, queue);
  } finally {
    queue.running = false;
    if (queue.target !== undefined) handlers.reschedule();
    else handlers.idle?.(queue);
  }
}

/** How one keyed control writes, confirms and fails. */
export interface LatestValueWrite<T, R> {
  /** Send one value to the device. */
  write(value: T): Promise<R>;
  /** Observe each accepted write, newest last. */
  accept?(result: R, value: T): void;
  /** Runs once after the newest value has been written: the authoritative
   * read and the completion notice belong here, not after every step. */
  settle?(last: R | undefined): Promise<void> | void;
  error(error: unknown): void | Promise<void>;
}

type KeyedQueue<T> = LatestValueQueue<T> & {
  settling: boolean;
  failed: boolean;
  last?: unknown;
  handlers?: LatestValueWrite<T, unknown>;
};

type QueuedCommand = { run(): Promise<void>; error(error: unknown): void };

type AdmissionHold = { admission: DeviceMutationAdmission; token: DeviceMutationLease; setter: (pending: boolean) => void; hostToken: object; running: number; retiring: boolean };

function retireHold(hold: AdmissionHold): void {
  if (!hold.retiring || hold.running) return;
  hold.token.release();
  try { completeDeviceCommand(hold.setter, hold.hostToken); }
  catch (error) { try { console.error("Device writer pending release failed:", error); } catch { /* Diagnostic only. */ } }
}

export interface LatestValueWriterOptions {
  /** Stable physical application ownership; queued values share one token. */
  lifetime?: PresetLifetime;
  /** Hook/context replacement, distinct from physical command ownership. */
  resetKey?: unknown;
  commandSessionKey?: unknown;
  /** The app-wide device-command flag. Held while a value is being written
   * or a discrete command queued behind it is running. */
  pending: boolean;
  setPending(pending: boolean): void;
}

/**
 * Keyed latest-value writer for range-style controls (dials and sliders).
 *
 * Every step of a gesture is submitted; while one write is in flight the
 * newest step waits and older ones are replaced, so the gesture's final value
 * is always written. Writes are serialised across keys. The submitted value
 * stays readable as a draft until its key has been written and settled, so a
 * controlled input follows the pointer instead of snapping back to the last
 * readback. A control must not disable itself while its own write is in
 * flight: callers expose `pending && !active` to their screens.
 */
export function useLatestValueWriter<T>(options: LatestValueWriterOptions) {
  const physicalKey = options.commandSessionKey ?? options.resetKey;
  const fallbackAdmission = useRef<{ key: unknown; admission: DeviceMutationAdmission } | undefined>(undefined);
  // Without a physical lifetime, an explicit replacement session supplies the
  // ownership boundary. Retiring work retains its old admission until settled;
  // it must not occupy the successor session's independent admission.
  if (!fallbackAdmission.current || (physicalKey !== undefined && !Object.is(fallbackAdmission.current.key, physicalKey))) {
    fallbackAdmission.current = { key: physicalKey, admission: new DeviceMutationAdmission() };
  }
  const admission = options.lifetime?.mutations ?? fallbackAdmission.current.admission;
  const activeHold = useRef<AdmissionHold | undefined>(undefined);
  const [, refreshWriter] = useState(0);
  const generation = useRef(0);
  const resetKey = useRef(options.resetKey);
  const visibleContext = useRef(options.resetKey);
  const retiringPending = useRef(false);
  const retiredWaiters = useRef<Array<() => void>>([]);
  const queues = useRef(new Map<string, KeyedQueue<T>>());
  const [drafts, setDrafts] = useState<ReadonlyMap<string, T>>(() => new Map());
  const [active, setActive] = useState(false);
  const activeRef = useRef(false);
  const mounted = useRef(true);
  const waiters = useRef<Array<() => void>>([]);
  const commands = useRef<QueuedCommand[]>([]);
  const commandRunning = useRef(false);
  const lease = useRef(0);
  const pendingRef = useRef(options.pending);
  const setPendingRef = useRef(options.setPending);
  const commandScope = useRef(options.setPending);
  bindDeviceCommandScope(options.setPending, commandScope.current);
  const pendingReset = useRef(false);
  if (physicalKey !== undefined) pendingReset.current = registerDeviceCommandSession(options.setPending, physicalKey, options.commandSessionKey !== undefined) || pendingReset.current;
  if (!Object.is(resetKey.current, options.resetKey)) {
    resetKey.current = options.resetKey;
    generation.current++;
    lease.current++;
    retiringPending.current ||= activeRef.current;
    for (const queue of queues.current.values()) queue.target = undefined;
    queues.current.clear(); commands.current = []; commandRunning.current = false;
    activeRef.current = false;
    const hold = activeHold.current;
    activeHold.current = undefined;
    if (hold) { hold.retiring = true; retireHold(hold); }
    retiredWaiters.current.push(...waiters.current.splice(0));
  }
  const renderGeneration = generation.current;
  pendingRef.current = retiringPending.current ? false : options.pending;
  setPendingRef.current = options.setPending;


  const retire = useCallback(retireHold, []);
  const own = useCallback(async <R,>(hold: AdmissionHold, work: () => Promise<R>): Promise<R> => {
    hold.running++;
    try { return await work(); }
    finally { hold.running--; retire(hold); }
  }, [retire]);
  const reserve = useCallback(() => {
    if (!mounted.current || renderGeneration !== generation.current) return false;
    if (activeRef.current) return Boolean(activeHold.current?.token.current());
    if (pendingRef.current || admission.busy || deviceCommandBusy(setPendingRef.current)) return false;
    const token = admission.tryAcquire();
    if (!token) return false;
    const setter = setPendingRef.current;
    const hostToken = {};
    if (!claimDeviceCommand(setter, hostToken)) { token.release(); return false; }
    activeHold.current = { admission, token, setter, hostToken, running: 0, retiring: false };
    return true;
  }, [admission, renderGeneration]);
  const retireActive = useCallback(() => {
    const hold = activeHold.current;
    activeHold.current = undefined;
    if (hold) { hold.retiring = true; retire(hold); }
  }, [retire]);


  const abandonUnstarted = useCallback((hold: AdmissionHold | undefined) => {
    if (!hold || activeHold.current !== hold || hold.running) return;
    // Only setup for this newly acquired, never-started token may be retired.
    lease.current++;
    queues.current.clear();
    commands.current = [];
    commandRunning.current = false;
    activeRef.current = false;
    activeHold.current = undefined;
    hold.retiring = true;
    try { if (mounted.current) { setActive(false); setDrafts(new Map()); } }
    finally { retire(hold); }
  }, [retire]);

  const setDraft = useCallback((key: string, value: T | undefined) => {
    if (!mounted.current) return;
    setDrafts((previous) => {
      if (value === undefined && !previous.has(key)) return previous;
      const next = new Map(previous);
      if (value === undefined) next.delete(key);
      else next.set(key, value);
      return next;
    });
  }, []);

  const busy = useCallback(() => {
    if (commandRunning.current) return true;
    for (const queue of queues.current.values()) if (queue.running || queue.settling) return true;
    return false;
  }, []);

  const hasValueWork = useCallback(() => {
    for (const queue of queues.current.values()) if (queue.running || queue.settling || queue.target !== undefined) return true;
    return false;
  }, []);

  const release = useCallback(() => {
    if (!activeRef.current) return;
    for (const queue of queues.current.values()) if (queue.running || queue.settling || queue.target !== undefined) return;
    activeRef.current = false;
    if (mounted.current) setActive(false);
    retireActive();
    for (const resolve of waiters.current.splice(0)) resolve();
  }, [retireActive]);

  const runRef = useRef<(key: string) => void>(() => {});
  const runCommandsRef = useRef<() => void>(() => {});

  const next = useCallback((preferred: string) => {
    const queue = queues.current.get(preferred);
    if (queue?.target !== undefined) { runRef.current(preferred); return; }
    for (const [key, candidate] of queues.current) {
      if (candidate.target !== undefined) { runRef.current(key); return; }
    }
    runCommandsRef.current();
  }, []);

  const settle = useCallback(async (key: string, queue: KeyedQueue<T>, owner: number, hold: AdmissionHold) => own(hold, async () => {
    if (owner !== lease.current) return;
    const failed = queue.failed;
    const last = queue.last;
    queue.failed = false;
    queue.last = undefined;
    if (!failed && mounted.current && queue.target === undefined && queue.handlers?.settle) {
      queue.settling = true;
      try { await queue.handlers.settle(last); }
      catch (error) { if (owner === lease.current) await queue.handlers.error(error); }
      finally { queue.settling = false; }
    }
    if (owner !== lease.current) return;
    if (queue.target === undefined) setDraft(key, undefined);
    if (!mounted.current) { queue.target = undefined; release(); return; }
    next(key);
  }), [next, own, release, setDraft]);

  runRef.current = (key: string) => {
    const queue = queues.current.get(key);
    if (!queue || queue.target === undefined || busy()) return;
    const owner = lease.current;
    const hold = activeHold.current;
    if (!hold?.token.current()) return;
    void own(hold, () => drainLatestValue(queue, {
      guard: () => owner === lease.current ? 0 : undefined,
      write: (target) => queue.handlers!.write(target),
      accept: (result, target) => {
        if (owner !== lease.current) return;
        queue.last = result;
        queue.handlers!.accept?.(result, target);
      },
      error: async (error) => {
        if (owner !== lease.current) return;
        queue.target = undefined;
        queue.failed = true;
        await queue.handlers!.error(error);
      },
      idle: () => void settle(key, queue, owner, hold),
      reschedule: () => { if (owner === lease.current) runRef.current(key); }
    }));
  };

  // Discrete mutations share this writer's ownership of the app-wide pending
  // flag. They wait behind every range value, then run one at a time; queued
  // callbacks cannot all wake from whenIdle() and dispatch concurrently.
  runCommandsRef.current = () => {
    if (commandRunning.current || hasValueWork()) return;
    const hold = activeHold.current;
    const command = commands.current.shift();
    if (!command) {
      release();
      return;
    }
    if (!hold?.token.current()) return;
    if (mounted.current) setActive(false);
    commandRunning.current = true;
    const owner = lease.current;
    void own(hold, () => Promise.resolve()
      .then(() => mounted.current && owner === lease.current ? command.run() : undefined)
      .catch((error) => { if (owner === lease.current) command.error(error); })
      .finally(() => {
        if (owner !== lease.current) return;
        commandRunning.current = false;
        runCommandsRef.current();
      }));
  };

  /** Queue `value` for `key`. Returns false, writing nothing, while another
   * device command owns the app-wide pending flag. */
  const submit = useCallback(<R>(key: string, value: T, handlers: LatestValueWrite<T, R>): boolean => {
    if (!mounted.current || commandRunning.current) return false;
    if (!reserve()) return false;
    const hold = activeHold.current;
    const acquiredHere = !activeRef.current;
    try {
      let queue = queues.current.get(key);
      if (!queue) {
        queue = { running: false, settling: false, failed: false };
        queues.current.set(key, queue);
      }
      queue.target = value;
      queue.handlers = handlers as LatestValueWrite<T, unknown>;
      setDraft(key, value);
      if (!activeRef.current) {
        activeRef.current = true;
        setActive(true);
        setPendingRef.current(true);
      }
      runRef.current(key);
      return true;
    } catch (error) {
      if (acquiredHere) abandonUnstarted(hold);
      throw error;
    }
  }, [abandonUnstarted, reserve, setDraft]);

  /** Admit a discrete mutation behind this writer's pending range updates.
   * Concurrent mutations accepted during the same drag are serialized. A
   * different app command still owns the shared pending flag and is refused. */
  const runCommand = useCallback((run: () => Promise<void>, error: (error: unknown) => void): boolean => {
    if (!mounted.current || !reserve()) return false;
    const hold = activeHold.current;
    const acquiredHere = !activeRef.current;
    try {
      commands.current.push({ run, error });
      if (!activeRef.current) {
        activeRef.current = true;
        setPendingRef.current(true);
      }
      runCommandsRef.current();
      return true;
    } catch (error) {
      if (acquiredHere) abandonUnstarted(hold);
      throw error;
    }
  }, [abandonUnstarted, reserve]);

  /** Resolves once every submitted value has been written and settled. */
  const whenIdle = useCallback(() => activeRef.current
    ? new Promise<void>((resolve) => { waiters.current.push(resolve); })
    : Promise.resolve(), []);

  const isActive = useCallback(() => activeRef.current, []);
  /** Cancel only this writer's lease; old completions cannot release a new one. */
  const cancel = useCallback(() => {
    lease.current += 1;
    for (const queue of queues.current.values()) queue.target = undefined;
    queues.current.clear();
    commands.current = [];
    commandRunning.current = false;
    const owned = activeRef.current;
    activeRef.current = false;
    if (mounted.current) { setActive(false); setDrafts(new Map()); }
    if (owned) retireActive();
    for (const resolve of waiters.current.splice(0)) resolve();
  }, [retireActive]);

  useEffect(() => {
    visibleContext.current = options.resetKey;
    retiringPending.current = false;
    setDrafts(new Map()); setActive(activeRef.current);
    for (const resolve of retiredWaiters.current.splice(0)) resolve();
    if (pendingReset.current) {
      pendingReset.current = false;
      if (!deviceCommandBusy(setPendingRef.current)) setPendingRef.current(false);
    }
  }, [options.resetKey, physicalKey]);

  useEffect(() => {
    let origin = options.lifetime?.checkpoint();
    return options.lifetime?.subscribe(() => {
      if (origin && !options.lifetime!.currentSession(origin)) { origin = options.lifetime!.checkpoint(); cancel(); generation.current++; refreshWriter((value) => value + 1); }
    });
  }, [cancel, options.lifetime]);

  useEffect(() => {
    const replayed = !mounted.current;
    mounted.current = true;
    if (replayed) refreshWriter((value) => value + 1);
    return () => {
      mounted.current = false;
      generation.current++;
      for (const queue of queues.current.values()) queue.target = undefined;
      commands.current = [];
      if (activeRef.current) {
        const inFlight = commandRunning.current || [...queues.current.values()].some((queue) => queue.running || queue.settling);
        if (!inFlight) {
          activeRef.current = false;
          retireActive();
        }
        for (const resolve of waiters.current.splice(0)) resolve();
      }
    };
  }, [retireActive]);

  const currentContext = Object.is(visibleContext.current, options.resetKey);
  return { submit, runCommand, drafts: currentContext ? drafts : new Map<string, T>(), active: currentContext && active, isActive, whenIdle, cancel };
}

export type LatestValueWriter<T> = ReturnType<typeof useLatestValueWriter<T>>;
