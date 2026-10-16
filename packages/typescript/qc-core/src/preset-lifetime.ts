import { samePresetAddress, type PresetAddress } from "./preset-address.ts";

export class SupersededPresetOperation extends Error {
  constructor() { super("The preset or device session changed before this operation completed."); }
}

export interface PresetOperation {
  readonly session: number;
  revision: number;
  target?: PresetAddress;
}

/** An admitted write owns its slot until its real asynchronous work settles. */
export interface DeviceMutationLease {
  current(): boolean;
  /** Returns true only when this release retired the current slot. */
  release(): boolean;
}

/**
 * Synchronous application admission, independent of React's pending render.
 * This does not cancel native work. A physical reset retires only session
 * ownership; an already-started old write still settles its own promise.
 */
export class DeviceMutationAdmission {
  private owner?: { scope: "session" | "host" };
  private readonly listeners = new Set<() => void>();
  get busy(): boolean { return this.owner !== undefined; }
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  snapshot = (): boolean => this.busy;
  tryAcquire(scope: "session" | "host" = "session"): DeviceMutationLease | undefined {
    if (this.owner) return undefined;
    const owner = { scope };
    this.owner = owner;
    this.notify();
    // A synchronous listener may reset/replace this physical ownership.
    if (this.owner !== owner) return undefined;
    return {
      current: () => this.owner === owner,
      release: () => {
        if (this.owner !== owner) return false;
        this.owner = undefined;
        this.notify();
        return true;
      }
    };
  }
  /** Local file saves survive a physical reconnect; old device slots do not. */
  resetSession(): void {
    if (this.owner?.scope !== "session") return;
    this.owner = undefined;
    this.notify();
  }
  private notify(): void {
    // Subscription changes belong to the next publication, never this iteration.
    for (const listener of [...this.listeners]) {
      try { listener(); }
      catch (error) {
        // Reporting cannot strand an already committed slot or skip peers.
        try { console.error("Device mutation admission listener failed:", error); } catch { /* Diagnostic only. */ }
      }
    }
  }
}

/**
 * Owns application work across authoritative preset transitions and reconnects.
 * Preview state does not advance this lifetime. A recall may adopt its one
 * expected target observation; unrelated transitions always revoke its work.
 */
export class PresetLifetime {
  readonly mutations = new DeviceMutationAdmission();
  private session = 0;
  private revision = 0;
  private address: PresetAddress;
  private readonly operations = new Set<PresetOperation>();
  private readonly listeners = new Set<() => void>();

  constructor(initial: PresetAddress) {
    this.address = { setlistKey: initial.setlistKey, presetPosition: initial.presetPosition };
  }

  get version(): number { return this.revision; }

  begin(target?: PresetAddress): PresetOperation {
    const operation = { session: this.session, revision: this.revision, target: target && { ...target } };
    this.operations.add(operation);
    return operation;
  }

  checkpoint(): PresetOperation {
    return {session: this.session, revision: this.revision};
  }

  currentSession(operation: PresetOperation): boolean {
    return operation.session === this.session;
  }

  current(operation: PresetOperation): boolean {
    return operation.session === this.session && operation.revision === this.revision;
  }

  assertCurrent(operation: PresetOperation): void {
    if (!this.current(operation)) throw new SupersededPresetOperation();
  }

  end(operation: PresetOperation): void {
    this.operations.delete(operation);
  }

  /** A serialized navigation chain can authorize its next step after readback. */
  follow(operation: PresetOperation, target: PresetAddress): void {
    this.assertCurrent(operation);
    operation.target = { ...target };
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  }

  observe(next: PresetAddress): void {
    if (samePresetAddress(this.address, next)) return;
    this.address = { ...next };
    const previous = this.revision++;
    for (const operation of this.operations) {
      if (operation.session === this.session && operation.revision === previous
        && operation.target && samePresetAddress(operation.target, next)) {
        operation.revision = this.revision;
        operation.target = undefined;
      }
    }
    this.notify();
  }

  /** Explicit same-address replacement also revokes work from the old content. */
  invalidate(): void {
    this.revision++;
    this.notify();
  }

  reset(): void {
    // Commit the complete physical/preset generation before either channel
    // publishes. A mutation listener may synchronously capture the new origin.
    this.session++;
    this.revision++;
    this.mutations.resetSession();
    this.notify();
  }

  private notify(): void {
    for (const listener of this.listeners) listener();
  }
}
