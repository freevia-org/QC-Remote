import type { PresetSnapshot } from "@qc-remote/client";

export interface GridSnapshotRefreshSequence {
  current: number;
}

export interface GridRepairCoordinator {
  producer: string | undefined;
  invalidationSequence: number | undefined;
  completedInvalidationSequence: number | undefined;
  attempted: boolean;
  inFlight: Promise<unknown> | undefined;
  episode: number;
  inFlightEpisode: number | undefined;
}

export interface GridRepairEpisode {
  producer: string | undefined;
  invalidationSequence: number | undefined;
  episode: number;
}

export function createGridRepairCoordinator(): GridRepairCoordinator {
  return {producer: undefined, invalidationSequence: undefined, completedInvalidationSequence: undefined, attempted: false, inFlight: undefined, episode: 0, inFlightEpisode: undefined};
}

/** Admit one repair for a topology episode shared by live events and recovery polling. */
export function admitGridRepair(
  coordinator: GridRepairCoordinator,
  producer: string | undefined,
  invalidationSequence: number | undefined
): boolean {
  if (coordinator.producer !== producer) {
    coordinator.episode += 1;
    coordinator.producer = producer;
    coordinator.invalidationSequence = invalidationSequence;
    coordinator.completedInvalidationSequence = undefined;
    coordinator.attempted = false;
  } else if (invalidationSequence !== undefined) {
    if (coordinator.completedInvalidationSequence !== undefined
        && invalidationSequence <= coordinator.completedInvalidationSequence) return false;
    if (coordinator.invalidationSequence !== undefined && invalidationSequence < coordinator.invalidationSequence) return false;
    if (invalidationSequence !== coordinator.invalidationSequence) {
      coordinator.episode += 1;
      coordinator.invalidationSequence = invalidationSequence;
      coordinator.completedInvalidationSequence = undefined;
      coordinator.attempted = false;
    }
  }
  if (invalidationSequence !== undefined && invalidationSequence === coordinator.completedInvalidationSequence) return false;
  if (coordinator.attempted) return false;
  coordinator.attempted = true;
  return true;
}

export function observeCompleteGrid(coordinator: GridRepairCoordinator, expected?: GridRepairEpisode): void {
  if (expected && (coordinator.producer !== expected.producer
      || coordinator.invalidationSequence !== expected.invalidationSequence
      || coordinator.episode !== expected.episode)) return;
  if (coordinator.invalidationSequence !== undefined) {
    coordinator.completedInvalidationSequence = coordinator.invalidationSequence;
  }
  coordinator.attempted = false;
}

export function runGridRepairOnce<T>(
  coordinator: GridRepairCoordinator,
  producer: string | undefined,
  invalidationSequence: number | undefined,
  operation: () => Promise<T>
): Promise<T | undefined> {
  if (!admitGridRepair(coordinator, producer, invalidationSequence)) return Promise.resolve(undefined);
  const attemptEpisode = coordinator.episode;
  if (coordinator.inFlight) {
    if (coordinator.inFlightEpisode !== attemptEpisode) coordinator.attempted = false;
    return Promise.resolve(undefined);
  }
  coordinator.inFlightEpisode = attemptEpisode;
  const wrapped = Promise.resolve().then(operation).finally(() => {
    if (coordinator.inFlight === wrapped) {
      coordinator.inFlight = undefined;
      coordinator.inFlightEpisode = undefined;
      if (coordinator.episode !== attemptEpisode) coordinator.attempted = false;
    }
  });
  coordinator.inFlight = wrapped;
  return wrapped;
}

export interface GridSnapshotRefreshOptions {
  sequence: GridSnapshotRefreshSequence;
  readSnapshot(read: () => Promise<PresetSnapshot>): Promise<PresetSnapshot>;
  read(): Promise<PresetSnapshot>;
  isCurrent(): boolean;
  onPending(): void;
  onError(error: unknown): void;
  onFinished(): void;
}

export function isIncompleteGridRefreshFailure(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /grid/i.test(message) && /incomplete/i.test(message);
}

/** Route event-triggered Grid refresh through the controller's guarded snapshot adoption. */
export async function refreshGridSnapshot(options: GridSnapshotRefreshOptions): Promise<boolean> {
  const request = ++options.sequence.current;
  options.onPending();
  try {
    const snapshot = await options.readSnapshot(async () => {
      const snapshot = await options.read();
      if (!options.isCurrent()) throw new Error("The active device session changed during Grid reconciliation.");
      return snapshot;
    });
    if (snapshot.gridLayoutComplete === false) throw new Error("The Grid layout is incomplete after refresh.");
    return true;
  } catch (error) {
    if (request === options.sequence.current && options.isCurrent()) options.onError(error);
    return false;
  } finally {
    if (request === options.sequence.current) options.onFinished();
  }
}

/** Shared event/timer repair admission. Any admitted failure consumes the episode. */
export function refreshGridSnapshotOnce(
  coordinator: GridRepairCoordinator,
  producer: string | undefined,
  invalidationSequence: number | undefined,
  options: GridSnapshotRefreshOptions
): Promise<void> {
  const result = runGridRepairOnce(coordinator, producer, invalidationSequence, async () => {
    const complete = await refreshGridSnapshot(options);
    return complete;
  });
  const admittedEpisode: GridRepairEpisode = {
    producer: coordinator.producer,
    invalidationSequence: coordinator.invalidationSequence,
    episode: coordinator.episode
  };
  return result.then((complete) => {
    if (complete) observeCompleteGrid(coordinator, admittedEpisode);
  });
}
