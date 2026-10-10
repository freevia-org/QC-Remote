import type { ModelPresetCatalogState, ModelPresetSummary, PresetSnapshot } from "@qc-remote/client";

/** What a client may know about the model presets the device announced in this session. */
export type GlobalEqPresets =
  | { status: "unavailable"; reason: GlobalEqPresetsUnavailable; invalidReason?: ModelPresetCatalogState["invalidReason"] }
  | { status: "observed"; presets: ModelPresetSummary[] };

export type GlobalEqPresetsUnavailable =
  /** The device has not announced a list in this session, or the host predates the fold. */
  | "not-observed"
  /** The host's fold may have lost a message or outgrown a limit; it stays unusable until the session ends. */
  | "invalidated"
  /** The projection names another session than the snapshot it travels with. */
  | "session-mismatch";

type CatalogCarrier = Pick<PresetSnapshot, "modelPresetCatalog" | "stateVersion">;

/**
 * Choose between the retained projection and a newer one within one state session.
 *
 * Each projection is the whole fold, so the latest wins, with two fences: a lower generation never
 * replaces a higher one, and an invalidated fold is never replaced inside its session, because the
 * host cannot prove the list whole again. A projection naming another session is ignored; the
 * caller has already dropped updates whose state version belongs to another session.
 */
export function adoptModelPresetCatalog(
  current: ModelPresetCatalogState | undefined,
  incoming: ModelPresetCatalogState | undefined
): ModelPresetCatalogState | undefined {
  if (!incoming) return current;
  // A projection that names no session cannot be fenced to one, so it is never adopted.
  if (!incoming.session) return current;
  if (!current) return incoming;
  if (current.session !== incoming.session) return current;
  if (current.state === "invalidated") return current;
  return incoming.generation >= current.generation ? incoming : current;
}

/**
 * The Global EQ presets a client may offer: those the host's fold lists for its scope hash, with a
 * complete and unique identity. The list is the fold's own account of what was announced, never
 * proof that nothing else exists, so an empty observed list is not "none exist".
 *
 * The projection counts only while it names the same state session as the snapshot that carries
 * it. A snapshot whose version moved to another session while an old projection stayed attached
 * is therefore unavailable, not stale-but-offered.
 */
export function globalEqPresetsFrom(snapshot: CatalogCarrier | undefined): GlobalEqPresets {
  return globalEqPresetsFor(snapshot?.modelPresetCatalog, snapshot?.stateVersion?.session);
}

/**
 * The same decision from the two values it depends on, so a caller can memoise on exactly those.
 * Fail closed: a missing or empty session on either side is a mismatch, never a match.
 */
export function globalEqPresetsFor(catalog: ModelPresetCatalogState | undefined, session: string | undefined): GlobalEqPresets {
  if (!catalog || catalog.state === "unavailable") return { status: "unavailable", reason: "not-observed" };
  if (typeof session !== "string" || !session || typeof catalog.session !== "string" || !catalog.session || catalog.session !== session) {
    return { status: "unavailable", reason: "session-mismatch" };
  }
  if (catalog.state === "invalidated") return { status: "unavailable", reason: "invalidated", invalidReason: catalog.invalidReason };
  const seen = new Map<string, number>();
  const candidates = (catalog.entries ?? []).filter((entry) =>
    entry.modelHash === catalog.scopeHash && entry.id.trim().length > 0 && typeof entry.isFactory === "boolean");
  for (const entry of candidates) {
    const key = tupleKey(entry);
    seen.set(key, (seen.get(key) ?? 0) + 1);
  }
  return { status: "observed", presets: candidates.filter((entry) => seen.get(tupleKey(entry)) === 1) };
}

/**
 * A copy of a snapshot without the device's model preset list. The list belongs to one device
 * session and is never persisted, exported or replayed (a saved workspace, an exported preset).
 */
export function withoutModelPresetCatalog<T extends { modelPresetCatalog?: unknown }>(snapshot: T): Omit<T, "modelPresetCatalog"> {
  const { modelPresetCatalog: _discarded, ...rest } = snapshot;
  return rest;
}

/** Why no preset list is offered, in words that never claim the device has no presets. */
export function globalEqUnavailableNotice(unavailable: Extract<GlobalEqPresets, { status: "unavailable" }>): string {
  if (unavailable.reason === "invalidated") {
    // The host's two size limits are inclusive maxima: a list of exactly that size is accepted and
    // only a strictly larger one invalidates the catalog. The numbers are the host's, not repeated here.
    return unavailable.invalidReason === "retention-limit" || unavailable.invalidReason === "publication-limit"
      ? "The Global EQ preset catalog is unavailable for this session because it holds more presets than the supported maximum. A list of exactly the maximum is accepted."
      : "The Global EQ preset catalog is unavailable for the rest of this session because it may be incomplete.";
  }
  return "The Global EQ preset catalog has not been observed in this session.";
}

/**
 * Why a recall must not be attempted, or undefined when this client may ask the host to try it.
 * The host repeats the check against its own fold and is the authority; this one keeps a stale or
 * foreign tuple from reaching the gateway at all.
 */
export function admitGlobalEqPreset(snapshot: CatalogCarrier | undefined, preset: ModelPresetSummary): string | undefined {
  const known = globalEqPresetsFrom(snapshot);
  if (known.status === "unavailable") return globalEqUnavailableNotice(known);
  const key = tupleKey(preset);
  return known.presets.some((candidate) => tupleKey(candidate) === key)
    ? undefined
    : "The Global EQ preset is not in the list the Quad Cortex announced in this session.";
}

function tupleKey(preset: Pick<ModelPresetSummary, "id" | "isFactory" | "modelHash">): string {
  return JSON.stringify([preset.modelHash, preset.isFactory, preset.id]);
}
