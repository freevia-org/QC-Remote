type Phase = "idle" | "searching" | "permission" | "opening" | "claiming" | "reader" | "handshake" | "synchronizing" | "ready" | "error" | "disconnected";
const order: Record<Phase, number> = {
  idle: -1, searching: 0, permission: 1, opening: 2, claiming: 3, reader: 4,
  handshake: 5, synchronizing: 6, ready: 100, error: 101, disconnected: 102
};

/** A terminal event belongs to its completed attempt, not the next open budget. */
export function advanceUsbConnectionProgress<T extends { requestId?: string; attempt?: number; phase: Phase }>(current: T, next: T): T {
  if (current.requestId !== next.requestId) return current;
  const nextAttempt = next.attempt ?? 0, currentAttempt = current.attempt ?? 0;
  if (nextAttempt < currentAttempt || (nextAttempt === currentAttempt && order[next.phase] < order[current.phase])) return current;
  return next;
}
