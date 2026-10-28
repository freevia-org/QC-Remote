export type AndroidStartupFailure = Readonly<{ sessionEpoch: number; detail: string }>;

/** Retain one failure, rejecting delayed older-session notifications. */
export function retainStartupFailure(current: AndroidStartupFailure | null, next: AndroidStartupFailure): AndroidStartupFailure | null {
  if (!Number.isSafeInteger(next.sessionEpoch) || next.sessionEpoch < 0
      || typeof next.detail !== "string" || next.detail.length === 0 || next.detail.length > 512) return current;
  if (current && current.sessionEpoch >= next.sessionEpoch) return current;
  return { sessionEpoch: next.sessionEpoch, detail: next.detail };
}

export function startupFailureReport(failure: AndroidStartupFailure | null): string {
  return failure ? `Last startup failure (session ${failure.sessionEpoch}; may precede the current retry): ${failure.detail}` : "";
}
