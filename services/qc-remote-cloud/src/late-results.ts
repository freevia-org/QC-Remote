/** A bounded record of requests whose deadline passed, so a result that arrives
 * afterwards is distinguishable from no result at all.
 *
 * The device session used to look a late result up in its pending map, find
 * nothing, and `return`. The caller kept a "the device did not respond in time"
 * error that the device had already disproved, and nothing anywhere recorded
 * that it had answered. That is the difference between a device which is slow
 * and a device which is gone, and the caller was shown the wrong one.
 */

export type LateResult = {
  /** The contract method whose deadline passed. */
  method: string;
  /** The deadline this worker gave it. */
  deadlineMs: number;
  /** How long after the deadline the device's own result turned up. */
  arrivedAfterDeadlineMs: number;
  /** Whether the device reported success. A late `true` means the caller was
   * told a write had failed when it had in fact been applied. */
  ok: boolean;
};

type Abandoned<Owner> = { owner?: Owner; method: string; deadlineMs: number; abandonedAt: number };

/** Bound retained deadline history even when hosts dispatch concurrently or
 * repeatedly disconnect. Owners are retained only within this bounded map. */
const CAPACITY = 32;

/** Everything the log knows, in a shape `structuredClone` can store.
 *
 * The owner must therefore be something that survives being written down. A
 * WebSocket is not; the connection id in its attachment is, and attachments are
 * exactly what Durable Object hibernation preserves. */
export type LateResultState<Owner> = {
  abandoned: Array<[string, Abandoned<Owner>]>;
  late: LateResult[];
};

export class LateResultLog<Owner = undefined> {
  private abandoned = new Map<string, Abandoned<Owner>>();
  private late: LateResult[] = [];

  /** Hand the whole log to storage. Without this the log lives only in the
   * incarnation that recorded the deadline, and the eviction that follows a
   * timed-out request is precisely what this file exists to survive: the
   * request ends, nothing else arrives, the session hibernates, and the late
   * result then finds an empty map and is dropped — the original behaviour the
   * header above says was replaced. */
  snapshot(): LateResultState<Owner> {
    return { abandoned: [...this.abandoned], late: [...this.late] };
  }

  /** Take back a stored log, ignoring anything that is not the shape we wrote. */
  restore(state: LateResultState<Owner> | undefined | null): void {
    if (!state || !Array.isArray(state.abandoned) || !Array.isArray(state.late)) return;
    this.abandoned = new Map(state.abandoned.slice(-CAPACITY));
    this.late = state.late.slice(-CAPACITY);
  }

  /** Record that `requestId` passed its deadline and was rejected. */
  abandon(requestId: string, method: string, deadlineMs: number, now = Date.now(), owner?: Owner): void {
    if (this.abandoned.size >= CAPACITY) {
      const oldest = this.abandoned.keys().next();
      if (!oldest.done) this.abandoned.delete(oldest.value);
    }
    this.abandoned.set(requestId, { owner, method, deadlineMs, abandonedAt: now });
  }

  /** Record a result for a request that is no longer pending. Returns the late
   * arrival when the request was one this worker abandoned, and `undefined` when
   * the id was never ours — a duplicate reply, or a reply from a previous
   * incarnation of this session. */
  observe(requestId: string, ok: boolean, now = Date.now(), owner?: Owner): LateResult | undefined {
    const abandoned = this.abandoned.get(requestId);
    if (!abandoned || abandoned.owner !== owner) return undefined;
    this.abandoned.delete(requestId);
    const arrival: LateResult = {
      method: abandoned.method,
      deadlineMs: abandoned.deadlineMs,
      arrivedAfterDeadlineMs: Math.max(0, now - abandoned.abandonedAt),
      ok
    };
    this.late.push(arrival);
    if (this.late.length > CAPACITY) this.late.shift();
    return arrival;
  }

  /** Every late arrival still remembered, oldest first. Reported by the session
   * status endpoint: a non-empty list means callers have been told the device
   * did not respond when it did, and the deadlines are too short. */
  arrivals(): LateResult[] {
    return [...this.late];
  }
}
