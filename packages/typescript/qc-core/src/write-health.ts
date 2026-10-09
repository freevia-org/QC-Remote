/** Track a session's write quarantine without conflating it with connection loss. */
export function transitionWriteQuarantine(previous: boolean, current: boolean): { quarantined: boolean; began: boolean } {
  return { quarantined: current, began: current && !previous };
}
