/**
 * The connection log is shown on screen and copied into diagnostics, and it
 * grows for as long as the window stays open (a broker that never
 * synchronizes is retried every minute). It keeps the newest entries only.
 */
export const MAX_CONNECTION_EVENTS = 500;

/** How many of the newest entries a diagnostics report carries. */
export const DIAGNOSTICS_EVENT_COUNT = 100;

export function appendConnectionEvent<Entry>(events: readonly Entry[], entry: Entry, max = MAX_CONNECTION_EVENTS): Entry[] {
  const kept = events.length >= max ? events.slice(events.length - (max - 1)) : events;
  return [...kept, entry];
}
