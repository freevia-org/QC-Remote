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

export interface LatestValueWriterOptions {
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
  const queues = useRef(new Map<string, KeyedQueue<T>>());
  const [drafts, setDrafts] = useState<ReadonlyMap<string, T>>(() => new Map());
  const [active, setActive] = useState(false);
  const activeRef = useRef(false);
  const mounted = useRef(true);
  const waiters = useRef<Array<() => void>>([]);
  const commands = useRef<QueuedCommand[]>([]);
  const commandRunning = useRef(false);
  const pendingRef = useRef(options.pending);
  const setPendingRef = useRef(options.setPending);
  pendingRef.current = options.pending;
  setPendingRef.current = options.setPending;

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
    setPendingRef.current(false);
    for (const resolve of waiters.current.splice(0)) resolve();
  }, []);

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

  const settle = useCallback(async (key: string, queue: KeyedQueue<T>) => {
    const failed = queue.failed;
    const last = queue.last;
    queue.failed = false;
    queue.last = undefined;
    if (!failed && mounted.current && queue.target === undefined && queue.handlers?.settle) {
      queue.settling = true;
      try { await queue.handlers.settle(last); }
      catch (error) { await queue.handlers.error(error); }
      finally { queue.settling = false; }
    }
    if (queue.target === undefined) setDraft(key, undefined);
    if (!mounted.current) { queue.target = undefined; release(); return; }
    next(key);
  }, [next, release, setDraft]);

  runRef.current = (key: string) => {
    const queue = queues.current.get(key);
    if (!queue || queue.target === undefined || busy()) return;
    void drainLatestValue(queue, {
      guard: () => 0,
      write: (target) => queue.handlers!.write(target),
      accept: (result, target) => {
        queue.last = result;
        queue.handlers!.accept?.(result, target);
      },
      error: async (error) => {
        queue.target = undefined;
        queue.failed = true;
        await queue.handlers!.error(error);
      },
      idle: () => void settle(key, queue),
      reschedule: () => runRef.current(key)
    });
  };

  // Discrete mutations share this writer's ownership of the app-wide pending
  // flag. They wait behind every range value, then run one at a time; queued
  // callbacks cannot all wake from whenIdle() and dispatch concurrently.
  runCommandsRef.current = () => {
    if (commandRunning.current || hasValueWork()) return;
    const command = commands.current.shift();
    if (!command) {
      release();
      return;
    }
    if (mounted.current) setActive(false);
    commandRunning.current = true;
    void Promise.resolve()
      .then(() => mounted.current ? command.run() : undefined)
      .catch(command.error)
      .finally(() => {
        commandRunning.current = false;
        runCommandsRef.current();
      });
  };

  /** Queue `value` for `key`. Returns false, writing nothing, while another
   * device command owns the app-wide pending flag. */
  const submit = useCallback(<R>(key: string, value: T, handlers: LatestValueWrite<T, R>): boolean => {
    if (!activeRef.current && pendingRef.current) return false;
    if (commandRunning.current) return false;
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
  }, [setDraft]);

  /** Admit a discrete mutation behind this writer's pending range updates.
   * Concurrent mutations accepted during the same drag are serialized. A
   * different app command still owns the shared pending flag and is refused. */
  const runCommand = useCallback((run: () => Promise<void>, error: (error: unknown) => void): boolean => {
    if (!mounted.current || (!activeRef.current && pendingRef.current)) return false;
    commands.current.push({ run, error });
    if (!activeRef.current) {
      activeRef.current = true;
      setPendingRef.current(true);
    }
    runCommandsRef.current();
    return true;
  }, []);

  /** Resolves once every submitted value has been written and settled. */
  const whenIdle = useCallback(() => activeRef.current
    ? new Promise<void>((resolve) => { waiters.current.push(resolve); })
    : Promise.resolve(), []);

  const isActive = useCallback(() => activeRef.current, []);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      for (const queue of queues.current.values()) queue.target = undefined;
      commands.current = [];
      if (activeRef.current) {
        const inFlight = commandRunning.current || [...queues.current.values()].some((queue) => queue.running || queue.settling);
        if (!inFlight) {
          activeRef.current = false;
          setPendingRef.current(false);
        }
        for (const resolve of waiters.current.splice(0)) resolve();
      }
    };
  }, []);

  return { submit, runCommand, drafts, active, isActive, whenIdle };
}

export type LatestValueWriter<T> = ReturnType<typeof useLatestValueWriter<T>>;
