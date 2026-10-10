import type { PresetSnapshot, QcStateUpdate } from "@qc-remote/client";

export type QcInteractionTarget =
  | { kind: "master-volume" }
  | { kind: "tempo" }
  | { kind: "parameter"; row: number; column: number; parameterIndex: number };

type Lease = {
  active: boolean;
  target: number;
  observed?: number;
  releasedAt?: number;
};

const RELEASE_FALLBACK_MS = 2_500;
const sameValue = (left: number, right: number) => Math.abs(left - right) <= .00001;

function keyOf(target: QcInteractionTarget): string {
  return target.kind === "parameter"
    ? `parameter:${target.row}:${target.column}:${target.parameterIndex}`
    : target.kind;
}

export function interactionObservationOf(state: QcStateUpdate): { target: QcInteractionTarget; value: number } | undefined {
  if (state.kind === "master" && state.masterVolume !== undefined) {
    // Type-17 carries the physical pot as a normalized float, while snapshots
    // and the public gateway expose Master Volume as an integer percentage.
    // Normalize at the event boundary so a pushed 0.57 can acknowledge a
    // queued target of 57 and can never pull the visible knob back to 0.57.
    const value = state.masterVolume >= 0 && state.masterVolume <= 1
      ? Math.round(state.masterVolume * 100)
      : state.masterVolume;
    return { target: { kind: "master-volume" }, value };
  }
  if ((state.kind === "tempo" || state.kind === "preset") && state.tempo !== undefined) {
    return { target: { kind: "tempo" }, value: state.tempo };
  }
  if (state.kind === "parameter" && state.row !== undefined && state.column !== undefined
      && state.parameterIndex !== undefined && state.normalizedValue !== undefined) {
    return {
      target: { kind: "parameter", row: state.row, column: state.column, parameterIndex: state.parameterIndex },
      value: state.normalizedValue
    };
  }
  return undefined;
}

/**
 * Arbitrates between a local gesture and asynchronous device observations.
 * The local target owns its value while the gesture is active and after
 * release until that exact value is observed. A bounded fallback prevents a
 * lost acknowledgement from permanently hiding later physical interaction.
 */
export class QcInteractionAuthority {
  private readonly leases = new Map<string, Lease>();

  begin(target: QcInteractionTarget, value: number): void {
    this.leases.set(keyOf(target), { active: true, target: value });
  }

  update(target: QcInteractionTarget, value: number): void {
    const key = keyOf(target);
    const current = this.leases.get(key);
    if (current) {
      current.active = true;
      current.target = value;
      current.releasedAt = undefined;
      return;
    }
    this.begin(target, value);
  }

  release(target: QcInteractionTarget, value: number, now = Date.now()): void {
    const key = keyOf(target);
    const current = this.leases.get(key) ?? { active: true, target: value };
    current.active = false;
    current.target = value;
    current.releasedAt = now;
    if (current.observed !== undefined && sameValue(current.observed, value)) this.leases.delete(key);
    else this.leases.set(key, current);
  }

  cancel(target: QcInteractionTarget): number | undefined {
    const key = keyOf(target);
    const observed = this.leases.get(key)?.observed;
    this.leases.delete(key);
    return observed;
  }

  reset(): void {
    this.leases.clear();
  }

  accepts(target: QcInteractionTarget, value: number, now = Date.now()): boolean {
    const key = keyOf(target);
    const lease = this.leases.get(key);
    if (!lease) return true;
    lease.observed = value;
    if (lease.active) return false;
    if (sameValue(value, lease.target)
        || (lease.releasedAt !== undefined && now - lease.releasedAt >= RELEASE_FALLBACK_MS)) {
      this.leases.delete(key);
      return true;
    }
    return false;
  }

  acceptsState(state: QcStateUpdate, now = Date.now()): boolean {
    const observation = interactionObservationOf(state);
    return !observation || this.accepts(observation.target, observation.value, now);
  }

  protectSnapshot(current: PresetSnapshot, incoming: PresetSnapshot, now = Date.now()): PresetSnapshot {
    const masterVolume = this.accepts({ kind: "master-volume" }, incoming.masterVolume, now)
      ? incoming.masterVolume : current.masterVolume;
    const tempo = this.accepts({ kind: "tempo" }, incoming.tempo, now)
      ? incoming.tempo : current.tempo;
    return masterVolume === incoming.masterVolume && tempo === incoming.tempo
      ? incoming
      : { ...incoming, masterVolume, tempo };
  }
}
