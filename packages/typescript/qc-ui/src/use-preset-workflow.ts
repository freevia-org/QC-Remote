import { requirePresetIdentity } from "@qc-remote/client";
import { useCallback, useEffect, useRef, useState } from "react";
import type { GatewayTransport, PresetEntry, PresetList, PresetSnapshot, SavePresetResult } from "@qc-remote/client";
import { canAdoptNativeState, samePresetAddress, type PresetLifetime, type PresetOperation } from "@qc-remote/core";
import { EMPTY_DIRECTORY_LIBRARY, type DirectoryLibraryKind, type DirectoryLibraryState, type PresetDirectoryState } from "./directory-library-model.ts";
import type { PresetClipboard, PresetWorkflowController, UsePresetWorkflowOptions } from "./preset-workflow-types.ts";

export type { PresetClipboard, PresetWorkflowController, UsePresetWorkflowOptions } from "./preset-workflow-types.ts";

/** How often an open Directory re-reads its setlist from the broker. */
export const DIRECTORY_REVALIDATE_MS = 2_000;

/**
 * Shared preset-library and persistent-preset controller.
 *
 * Windows and Android provide only their gateway, connection state, prompts,
 * and presentation callbacks. All validation, sequencing, loading, clipboard,
 * and authoritative snapshot reconciliation lives here.
 */
export function usePresetWorkflow(options: UsePresetWorkflowOptions): PresetWorkflowController {
  const {
    gateway, snapshotRef, setSnapshot, connected, pending, setPending,
    prompts, notice, fail, onPresetChanged
  } = options;
  const [presetList, setPresetList] = useState<PresetList>();
  const [directoryOpen, setDirectoryOpen] = useState(false);
  const [presetListLoading, setPresetListLoading] = useState(false);
  const [presetFoldersLoading, setPresetFoldersLoading] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);
  const [saveName, setSaveName] = useState("");
  const [clipboard, setClipboard] = useState<PresetClipboard>();
  const [batchClipboard, setBatchClipboard] = useState<PresetClipboard[]>();
  /** Read from the unit's Directory indicator; unset until read, and then the signed-out drawing stands. */
  const [cloudSignedIn, setCloudSignedIn] = useState<boolean>();
  const [library, setLibrary] = useState<DirectoryLibraryState>(EMPTY_DIRECTORY_LIBRARY);
  const loadSequence = useRef(0);
  const foldersLoaded = useRef(false);
  const catalogWarmed = useRef(false);
  const saveTarget = useRef<{ snapshot: PresetSnapshot; operation?: PresetOperation } | undefined>(undefined);

  const closeSave = useCallback(() => {
    if (saveTarget.current?.operation) options.lifetime?.end(saveTarget.current.operation);
    saveTarget.current = undefined;
    setSaveOpen(false);
  }, [options.lifetime]);
  useEffect(() => options.lifetime?.subscribe(closeSave), [closeSave, options.lifetime]);
  useEffect(() => { if (!connected) closeSave(); }, [closeSave, connected]);
  useEffect(() => () => {
    if (saveTarget.current?.operation) options.lifetime?.end(saveTarget.current.operation);
  }, [options.lifetime]);

  const unavailable = useCallback((action: string) => {
    if (!connected) {
      notice(`Connect the Quad Cortex before ${action}.`);
      return true;
    }
    if (pending) {
      notice("A device command is already in progress.");
      return true;
    }
    return false;
  }, [connected, notice, pending]);

  // The Directory's non-preset categories read the device library straight
  // from the gateway. A failed read is kept per kind and drawn in its list
  // rather than raised as a notice: opening a category must not interrupt the
  // user, and an unpublished catalog is a state of the device, not a fault.
  const loadLibrary = useCallback((kind: DirectoryLibraryKind) => {
    if (!connected) return;
    setLibrary((current) => ({ ...current, loading: [...current.loading.filter((candidate) => candidate !== kind), kind], errors: { ...current.errors, [kind]: undefined } }));
    const read = kind === "favorites" ? gateway.favorites() : kind === "recents" ? gateway.recents() : kind === "captures" ? gateway.captures() : gateway.irs(null);
    read.then(
      (result) => setLibrary((current) => ({ ...current, [kind]: result, loading: current.loading.filter((candidate) => candidate !== kind) })),
      (error: unknown) => setLibrary((current) => ({ ...current, loading: current.loading.filter((candidate) => candidate !== kind), errors: { ...current.errors, [kind]: error instanceof Error ? error.message : String(error) } }))
    );
  }, [connected, gateway]);

  // Recent rows carry no date on the wire; the date is the preset's own, which a
  // setlist read already holds. Read each setlist a dateless Recent row names, at
  // most once per connection, from the broker's retained catalog (the same read
  // the open-setlist revalidation uses, so no device traffic and no new loop).
  // Failures are kept as "asked": a setlist that cannot be read is not retried
  // by every render.
  const [setlistDates, setSetlistDates] = useState<ReadonlyMap<string, string | number>>(() => new Map());
  const requestedSetlistDates = useRef(new Set<string>());
  const loadSetlistDates = useCallback((folderKeys: readonly string[]) => {
    if (!connected) return;
    const wanted = [...new Set(folderKeys)].filter((key) => key && !requestedSetlistDates.current.has(key));
    if (!wanted.length) return;
    for (const key of wanted) requestedSetlistDates.current.add(key);
    void (async () => {
      for (const key of wanted) {
        try {
          const list = await gateway.listPresets(false, key);
          if (!requestedSetlistDates.current.has(key)) return;
          const found = new Map<string, string | number>();
          for (const entry of list.presets) {
            const date = entry.dateMsSinceEpoch ?? (entry.date || undefined);
            if (entry.productKey && date !== undefined) found.set(entry.productKey, date);
          }
          if (found.size) setSetlistDates((current) => new Map([...current, ...found]));
        } catch {
          // The row keeps drawing without a date.
        }
      }
    })();
  }, [connected, gateway]);

  const commitPreset = useCallback((next: PresetSnapshot) => {
    if (!canAdoptNativeState(snapshotRef.current.stateVersion, next.stateVersion)) return snapshotRef.current;
    const adopted = options.acceptSnapshot ? options.acceptSnapshot(next) : next;
    if (!options.acceptSnapshot) {
      snapshotRef.current = adopted;
      setSnapshot(adopted);
    }
    onPresetChanged?.(adopted);
    return adopted;
  }, [onPresetChanged, options.acceptSnapshot, setSnapshot, snapshotRef]);

  const commitSavedPreset = useCallback((result: SavePresetResult, before: PresetSnapshot) => {
    // A verified catalog mutation does not prove that the edit buffer is still
    // clean. Native frames and local edits can both arrive while Save is pending.
    // A local edit can retain its native revision. Only replace an unchanged
    // dispatch observation here; the controller also rejects older native state.
    let next = snapshotRef.current;
    if (next === before) {
      next = { ...(result.snapshot ?? before), presetName: result.savedName };
      next = commitPreset(next);
    }
    const saved = result.snapshot ?? before;
    setPresetList((current) => current && current.setlistKey === saved.setlistKey
      ? {
          ...current,
          presets: current.presets.map((entry) => entry.position === saved.presetPosition
            ? { ...entry, name: result.savedName }
            : entry)
        }
      : current);
    return next;
  }, [commitPreset, snapshotRef]);

  const loadDirectory = useCallback(async (refresh = false, setlistKey?: string, quiet = false) => {
    const sequence = ++loadSequence.current;
    setPresetListLoading(true);
    try {
      const list = await gateway.listPresets(refresh, setlistKey);
      if (sequence === loadSequence.current) setPresetList(list);
      return list;
    } catch (error) {
      if (!quiet) fail(error);
      return undefined;
    } finally {
      if (sequence === loadSequence.current) setPresetListLoading(false);
    }
  }, [fail, gateway]);

  const loadFolders = useCallback(async (refresh = false, quiet = false) => {
    setPresetFoldersLoading(true);
    try {
      const result = await gateway.listPresetFolders(refresh);
      foldersLoaded.current = true;
      setPresetList((current) => current ? { ...current, folders: result.folders } : current);
    } catch (error) {
      if (!quiet) fail(error);
    } finally {
      setPresetFoldersLoading(false);
    }
  }, [fail, gateway]);

  // The QC's boot handshake already spends ~12.6 s pulling its whole directory,
  // and the broker keeps every setlist from that one publication. Opening the
  // browser used to flip the panel open and only then issue its first
  // listPresets, so the user got an empty frame and a wait for something the
  // host had already been given. Warm the browser from that publication as soon
  // as the device is ready; the open is then a render, not a round trip.
  //
  // Quiet on purpose: this is not a user action, and the boot pull's tail can
  // still be in flight when readiness lands.
  useEffect(() => {
    if (!connected) {
      catalogWarmed.current = false;
      return;
    }
    if (catalogWarmed.current || presetList) return;
    catalogWarmed.current = true;
    void (async () => {
      if (await loadDirectory(false, undefined, true)) await loadFolders(false, true);
      // Keep the attempt latched on failure. Loading-state renders and changing
      // callback identities must not turn one rejection into an unbounded loop.
      // Opening/refreshing the directory can retry; a new connection resets us.
    })();
  }, [connected, loadDirectory, loadFolders, presetList]);

  // The Directory is a cache of the broker's retained publication, and the
  // library is also written by routes this view never sees: the host's MCP
  // tools, a second client, the unit itself. Saving, renaming, copying, moving
  // and deleting through any of them left this list frozen at whatever it read
  // on open (a saved slot kept reading "Unsaved" for as long as the panel
  // stayed up) while the active-slot highlight, which follows the live
  // snapshot, moved on. Re-read the open setlist quietly: the read is served
  // from the broker's retained catalog, so it costs no device traffic, and the
  // list is replaced only when it actually differs so the rows do not churn.
  const presetListRef = useRef<PresetList | undefined>(presetList);
  presetListRef.current = presetList;
  const revalidating = useRef(false);
  const revalidateDirectory = useCallback(async () => {
    const key = presetListRef.current?.setlistKey;
    if (!key || revalidating.current) return;
    revalidating.current = true;
    const sequence = loadSequence.current;
    try {
      const fresh = await gateway.listPresets(false, key);
      if (sequence !== loadSequence.current || fresh.loading) return;
      setPresetList((current) => current && current.setlistKey === fresh.setlistKey && JSON.stringify(current) !== JSON.stringify(fresh) ? fresh : current);
    } catch {
      // A failed background read keeps the list on screen; the next tick retries.
    } finally {
      revalidating.current = false;
    }
  }, [gateway]);
  useEffect(() => {
    if (!directoryOpen || !connected) return;
    void revalidateDirectory();
    const timer = setInterval(() => void revalidateDirectory(), DIRECTORY_REVALIDATE_MS);
    return () => clearInterval(timer);
  }, [connected, directoryOpen, revalidateDirectory]);

  const openDirectory = useCallback(async (reload = false) => {
    if (unavailable("opening its preset browser")) return;
    setDirectoryOpen(true);
    // `reload` re-reads this session's published catalog. It deliberately does
    // not set the gateway's `refresh`: the QC publishes its directory once per
    // USB session and answers no second directory read, so `refresh` fails
    // closed and a control built on it could only produce an error.
    if (reload || !presetList || presetList.setlistKey !== snapshotRef.current.setlistKey) {
      await loadDirectory(false, snapshotRef.current.setlistKey);
    }
    if (reload || !foldersLoaded.current) void loadFolders(false);
  }, [loadDirectory, loadFolders, presetList, snapshotRef, unavailable]);

  const recall = useCallback(async (entry: PresetEntry) => {
    const current = snapshotRef.current;
    if (!presetList || samePresetAddress({ setlistKey: presetList.setlistKey, presetPosition: entry.position }, current) || unavailable("recalling a preset")) return;
    setPending(true);
    notice(`Recalling ${entry.location} · ${entry.name}…`);
    try {
      const result = await gateway.recallPreset(presetList.setlistKey, entry.position, current.presetName, current.presetPosition, current.setlistKey, requirePresetIdentity(current));
      if (result.snapshot) commitPreset(result.snapshot);
      notice(result.detail ?? `${entry.location} · ${entry.name} recalled.`);
      setDirectoryOpen(false);
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [commitPreset, fail, gateway, notice, presetList, setPending, snapshotRef, unavailable]);

  const recallLocation = useCallback(async (location: string): Promise<string> => {
    const current = snapshotRef.current;
    if (!connected) throw new Error("Connect the Quad Cortex before recalling a preset.");
    if (pending) throw new Error("A device command is already in progress.");
    const list = presetList?.setlistKey === current.setlistKey && !presetList.loading
      ? presetList
      : await gateway.listPresets(false, current.setlistKey);
    const entry = list.presets.find((candidate) => candidate.location.toUpperCase() === location.toUpperCase());
    if (!entry) throw new Error(`${location.toUpperCase()} is empty in ${list.setlistName}.`);
    if (entry.position === current.presetPosition) return `${entry.location} · ${entry.name} is already active.`;
    setPending(true);
    try {
      const result = await gateway.recallPreset(list.setlistKey, entry.position, current.presetName, current.presetPosition, current.setlistKey, requirePresetIdentity(current));
      if (result.snapshot) commitPreset(result.snapshot);
      setDirectoryOpen(false);
      return result.detail ?? `${entry.location} · ${entry.name} recalled.`;
    } finally {
      setPending(false);
    }
  }, [commitPreset, connected, gateway, pending, presetList, setPending, snapshotRef]);

  const reload = useCallback(async () => {
    const current = snapshotRef.current;
    if (unavailable("discarding preset changes")) return;
    setPending(true);
    notice("Reloading the stored preset…");
    try {
      const result = await gateway.reloadPreset(current.presetName, current.presetPosition, requirePresetIdentity(current));
      if (result.snapshot) commitPreset(result.snapshot);
      notice(result.detail ?? "Stored preset reloaded.");
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [commitPreset, fail, gateway, notice, setPending, snapshotRef, unavailable]);

  const openSave = useCallback(() => {
    if (unavailable("saving a preset")) return;
    if (saveTarget.current?.operation) options.lifetime?.end(saveTarget.current.operation);
    saveTarget.current = {snapshot: snapshotRef.current, operation: options.lifetime?.begin()};
    setDirectoryOpen(false);
    setSaveName(snapshotRef.current.presetName === "Unsaved" ? "" : snapshotRef.current.presetName);
    setSaveOpen(true);
  }, [options.lifetime, snapshotRef, unavailable]);

  const save = useCallback(async () => {
    const target = saveTarget.current;
    const current = target?.snapshot;
    const name = saveName.trim();
    if (!saveOpen || !name || unavailable("saving a preset")) return;
    if (!current || !samePresetAddress(current, snapshotRef.current)
      || current.presetName !== snapshotRef.current.presetName
      || (target.operation && !options.lifetime?.current(target.operation))) {
      closeSave();
      notice("The active preset changed. Open Save again for the preset you want to save.");
      return;
    }
    setPending(true);
    notice(`Saving preset to ${current.presetLocation}…`);
    const beforeSave = snapshotRef.current;
    try {
      const result = await gateway.savePresetAs(
        current.setlistKey,
        current.presetPosition,
        name,
        current.presetName,
        current.presetPosition,
        current.presetName !== "Unsaved"
      , requirePresetIdentity(current));
      commitSavedPreset(result, beforeSave);
      notice(result.detail ?? `${current.presetLocation} saved.`);
      closeSave();
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [closeSave, commitSavedPreset, fail, gateway, notice, options.lifetime, saveName, saveOpen, setPending, snapshotRef, unavailable]);

  const saveCurrentUnsaved = useCallback(async (requestedName: string): Promise<string> => {
    const current = snapshotRef.current;
    const name = requestedName.trim();
    if (!connected) throw new Error("Connect the Quad Cortex before saving a device preset.");
    if (pending) throw new Error("A device command is already in progress.");
    if (!name) throw new Error("A preset name is required for device save.");
    if (current.presetName !== "Unsaved") throw new Error("The active preset is already stored. Use Save As or Rename for an occupied slot.");
    setPending(true);
    notice(`Saving preset to ${current.presetLocation}…`);
    const beforeSave = snapshotRef.current;
    try {
      const result = await gateway.savePresetAs(
        current.setlistKey,
        current.presetPosition,
        name,
        current.presetName,
        current.presetPosition,
        false
      , requirePresetIdentity(current));
      commitSavedPreset(result, beforeSave);
      const detail = result.detail ?? `${current.presetLocation} saved.`;
      notice(detail);
      return detail;
    } finally {
      setPending(false);
    }
  }, [commitSavedPreset, connected, gateway, notice, pending, setPending, snapshotRef]);

  const copy = useCallback(() => {
    const current = snapshotRef.current;
    if (unavailable("copying a preset")) return;
    if (current.dirty) {
      notice("Save or discard the current changes before copying this preset.");
      return;
    }
    if (current.presetName === "Unsaved") {
      notice("The current slot does not contain a stored preset to copy.");
      return;
    }
    setClipboard({
      setlistKey: current.setlistKey,
      setlistName: current.setlistName,
      presetPosition: current.presetPosition,
      presetLocation: current.presetLocation,
      presetName: current.presetName,
      mode: "copy"
    });
    notice(`Copied preset ${current.presetLocation} · ${current.presetName}. Navigate to a user preset slot and choose Paste Preset.`);
  }, [notice, snapshotRef, unavailable]);

  const paste = useCallback(async () => {
    const current = snapshotRef.current;
    if (!clipboard) {
      notice("Copy a stored preset before pasting.");
      return;
    }
    if (clipboard.mode === "cut") {
      notice("The cut preset is ready in Preset Directory. Choose a destination there to move or swap it.");
      return;
    }
    if (unavailable("pasting a preset")) return;
    if (current.dirty) {
      notice("Save or discard the destination's unsaved changes before pasting a preset.");
      return;
    }
    if (clipboard.setlistKey === current.setlistKey && clipboard.presetPosition === current.presetPosition) {
      notice("The copied preset is already loaded in this slot.");
      return;
    }
    setPending(true);
    notice(`Copying ${clipboard.presetLocation} · ${clipboard.presetName} to ${current.presetLocation}…`);
    try {
      // One File COPY frame, as Cortex Control sends it. It names the source
      // by file key and the destination by slot; it neither recalls nor saves,
      // so the active preset snapshot is not rewritten from its result.
      const result = await gateway.copyPreset(
        clipboard.setlistKey,
        clipboard.presetPosition,
        clipboard.presetName,
        current.setlistKey,
        current.presetPosition,
        true
      );
      // The mutation's own verified catalog state is already in the
      // broker. Re-read it; asking for a device refresh here blocked the
      // whole UI for 45 seconds and then failed quietly.
      await loadDirectory(false, current.setlistKey, true);
      notice(result.detail ?? `${current.presetLocation} replaced.`);
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [clipboard, fail, gateway, loadDirectory, notice, setPending, snapshotRef, unavailable]);

  const rename = useCallback(async (suggestedName = snapshotRef.current.presetName) => {
    const current = snapshotRef.current;
    if (unavailable("renaming a preset")) return;
    const name = (await prompts.prompt("New preset name", suggestedName))?.trim();
    if (!name || name === current.presetName) return;
    setPending(true);
    notice(`Renaming ${current.presetLocation}…`);
    const beforeSave = snapshotRef.current;
    try {
      const result = await gateway.renameCurrentPreset(name, current.presetName, current.presetPosition, true, requirePresetIdentity(current));
      commitSavedPreset(result, beforeSave);
      // The mutation's own verified catalog state is already in the
      // broker. Re-read it; asking for a device refresh here blocked the
      // whole UI for 45 seconds and then failed quietly.
      await loadDirectory(false, current.setlistKey, true);
      notice(result.detail ?? `${current.presetLocation} renamed.`);
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [commitSavedPreset, fail, gateway, loadDirectory, notice, prompts, setPending, snapshotRef, unavailable]);

  const directoryPresetAction = useCallback(async (action: "upload" | "edit" | "copy" | "cut" | "paste", entry: PresetEntry) => {
    const list = presetList;
    if (action === "upload") {
      notice(`Upload to Cloud for ${entry.location} · ${entry.name} is not exposed by the current USB gateway.`);
      return;
    }
    if (action === "edit") {
      if (list?.setlistKey === snapshotRef.current.setlistKey && entry.position === snapshotRef.current.presetPosition) {
        void rename(entry.name);
      } else {
        notice(`Recall ${entry.location} · ${entry.name} before renaming it.`);
      }
      return;
    }
    if (action === "copy" || action === "cut") {
      if (!list || entry.occupied === false || entry.name === "Unsaved") {
        notice("An empty preset slot cannot be copied or cut.");
        return;
      }
      setClipboard({
        setlistKey: list.setlistKey,
        setlistName: list.setlistName,
        presetPosition: entry.position,
        presetLocation: entry.location,
        presetName: entry.name,
        mode: action
      });
      setBatchClipboard(undefined);
      notice(`${action === "copy" ? "Copied" : "Cut"} ${entry.location} · ${entry.name}. Select a destination and choose Paste.`);
      return;
    }

    if (action === "paste" && batchClipboard?.length) {
      if (!list) { notice("Load a preset setlist before pasting."); return; }
      if (unavailable("pasting presets")) return;
      const ordered = [...batchClipboard].sort((left, right) => left.presetPosition - right.presetPosition);
      const destinationPositions = ordered.map((_, index) => entry.position + index);
      if (destinationPositions.some((position) => position > 255)) {
        notice("The selected presets do not fit from this destination slot.");
        return;
      }
      const overlappingSource = ordered.find((source) => source.setlistKey === list.setlistKey && destinationPositions.includes(source.presetPosition));
      if (overlappingSource) {
        notice(`The destination range overlaps selected source ${overlappingSource.presetLocation}. Choose a non-overlapping first destination slot.`);
        return;
      }
      setPending(true);
      let completed = 0;
      try {
        const destinationList = await gateway.listPresets(false, list.setlistKey);
        const sourcesBySetlist = new Map<string, PresetList>();
        for (const source of ordered) {
          if (!sourcesBySetlist.has(source.setlistKey)) sourcesBySetlist.set(source.setlistKey, await gateway.listPresets(false, source.setlistKey));
          const freshSource = sourcesBySetlist.get(source.setlistKey)?.presets.find((candidate) => candidate.position === source.presetPosition);
          if (!freshSource || freshSource.occupied === false || freshSource.name !== source.presetName) throw new Error(`${source.presetLocation} changed or disappeared. Refresh the Directory before retrying.`);
        }
        const overwritten = destinationPositions.map((position) => destinationList.presets.find((candidate) => candidate.position === position)).filter((candidate) => candidate?.occupied !== false && candidate?.name !== "Unsaved");
        if (overwritten.length && !await prompts.confirm(`Replace ${overwritten.length} occupied preset slot${overwritten.length === 1 ? "" : "s"} with ${ordered.length} copied preset${ordered.length === 1 ? "" : "s"}?`)) return;
        notice(`Copying ${ordered.length} presets…`);
        for (let index = 0; index < ordered.length; index += 1) {
          const source = ordered[index];
          const position = destinationPositions[index];
          const destination = destinationList.presets.find((candidate) => candidate.position === position);
          await gateway.copyPreset(source.setlistKey, source.presetPosition, source.presetName, list.setlistKey, position, destination?.occupied !== false && destination?.name !== "Unsaved");
          completed += 1;
        }
        const refreshed = await loadDirectory(false, list.setlistKey, true);
        const verified = refreshed && ordered.every((source, index) => refreshed.presets.find((candidate) => candidate.position === destinationPositions[index])?.name === source.presetName);
        if (!verified) throw new Error("The batch copy completed, but its catalog readback did not match every destination. Inspect the Directory before retrying.");
        setBatchClipboard(undefined);
        notice(`${ordered.length} presets copied and verified.`);
      } catch (error) {
        try { await loadDirectory(false, list.setlistKey, true); } catch { /* Preserve the operation error. */ }
        fail(completed ? new Error(`${completed} of ${ordered.length} presets were copied before the operation stopped. The Directory was refreshed; inspect it before retrying. ${error instanceof Error ? error.message : String(error)}`) : error);
      } finally { setPending(false); }
      return;
    }

    const source = clipboard;
    if (!source) { notice("Copy or cut a stored preset before pasting."); return; }
    if (!list) { notice("Load a preset setlist before pasting."); return; }
    if (entry.position === source.presetPosition && list.setlistKey === source.setlistKey) {
      notice("The source and destination are the same preset slot.");
      return;
    }
    if (unavailable("pasting a preset")) return;
    setPending(true);
    try {
      // Re-read every involved setlist immediately before the persistent
      // operation. The device catalog, not a stale React row, decides whether
      // the source still exists and whether the destination is occupied.
      const sourceList = await gateway.listPresets(false, source.setlistKey);
      const destinationList = source.setlistKey === list.setlistKey
        ? sourceList
        : await gateway.listPresets(false, list.setlistKey);
      const sourceEntry = sourceList.presets.find((candidate) => candidate.position === source.presetPosition);
      const destination = destinationList.presets.find((candidate) => candidate.position === entry.position);
      if (!sourceEntry || sourceEntry.occupied === false || sourceEntry.name !== source.presetName) {
        throw new Error("The copied preset changed or disappeared from the device catalog. Refresh the Directory before retrying.");
      }
      if (!destination) throw new Error("The destination slot is not present in the device catalog. Refresh the Directory before retrying.");
      if (source.mode === "cut") {
        if (source.setlistKey !== list.setlistKey) {
          throw new Error("Cut and paste can move presets only within one setlist. Use Copy to paste into another setlist.");
        }
        const occupied = destination.occupied && destination.name !== "Unsaved";
        const effect = occupied
          ? `swap ${source.presetLocation} · ${source.presetName} with ${destination.location} · ${destination.name}`
          : `move ${source.presetLocation} · ${source.presetName} to empty slot ${destination.location}`;
        if (!await prompts.confirm(`This is permanent: ${effect}?`)) return;
        notice(`${occupied ? "Swapping" : "Moving"} presets…`);
        await gateway.movePreset(list.setlistKey, source.presetPosition, source.presetName, destination.position);
        const refreshed = await loadDirectory(false, list.setlistKey, true);
        if (!refreshed) throw new Error("The move was sent, but the updated preset directory could not be verified. Refresh before doing anything else.");
        const moved = refreshed.presets.find((candidate) => candidate.position === destination.position);
        const sourceAfter = refreshed.presets.find((candidate) => candidate.position === source.presetPosition);
        const verified = moved?.name === source.presetName && moved.occupied !== false && (occupied
          ? sourceAfter?.name === destination.name
          : sourceAfter?.occupied === false || sourceAfter?.name === "Unsaved");
        if (!verified) throw new Error("The device accepted the move, but the catalog readback did not match the requested result. Do not retry until you inspect the Directory.");
        setClipboard(undefined);
        notice(occupied ? "Presets swapped and verified." : "Preset moved and verified.");
        return;
      }

      const occupied = destination.occupied !== false && destination.name !== "Unsaved";
      if (occupied && !await prompts.confirm(`Overwrite ${destination.location} · ${destination.name} with ${source.presetName}? This permanently replaces the destination preset.`)) return;
      // Host policy (not a Cortex Control frame): copy touches no active
      // preset, so unsaved edits only matter when the destination is the
      // active slot itself.
      const current = snapshotRef.current;
      if (current.dirty && current.setlistKey === list.setlistKey && current.presetPosition === destination.position) {
        throw new Error("Save or discard the active preset's unsaved changes before pasting over it.");
      }
      notice(`Copying ${source.presetLocation} · ${source.presetName} to ${destination.location}…`);
      // One File COPY frame, as Cortex Control sends it.
      const result = await gateway.copyPreset(
        source.setlistKey,
        source.presetPosition,
        source.presetName,
        list.setlistKey,
        destination.position,
        occupied
      );
      const refreshed = await loadDirectory(false, list.setlistKey, true);
      if (!refreshed || refreshed.presets.find((candidate) => candidate.position === destination.position)?.name !== source.presetName) {
        throw new Error("The copy was sent, but the destination could not be verified from the refreshed catalog. Do not retry until you inspect the Directory.");
      }
      setClipboard(undefined);
      notice(result.detail ?? `Preset copied to ${destination.location} and verified.`);
    } catch (error) {
      // A timeout may follow a completed persistent operation. Reconcile the
      // catalog once; never replay the mutation on an uncertain result.
      try { await loadDirectory(false, list.setlistKey, true); } catch { /* Preserve the operation error. */ }
      fail(error);
    } finally { setPending(false); }
  }, [batchClipboard, clipboard, fail, gateway, loadDirectory, notice, pending, presetList, prompts, rename, setPending, snapshotRef, unavailable]);

  const directoryPresetBatchAction = useCallback(async (action: "copy" | "delete", entries: PresetEntry[]) => {
    const list = presetList;
    const stored = entries.filter((entry) => entry.occupied !== false && entry.name !== "Unsaved");
    if (!list || !stored.length) return;
    if (action === "copy") {
      setClipboard(undefined);
      setBatchClipboard(stored.map((entry) => ({
        setlistKey: list.setlistKey,
        setlistName: list.setlistName,
        presetPosition: entry.position,
        presetLocation: entry.location,
        presetName: entry.name,
        mode: "copy"
      })));
      notice(`${stored.length} presets copied. Choose the first destination slot and Paste.`);
      return;
    }
    if (unavailable("deleting presets")) return;
    if (!await prompts.confirm(`Permanently delete ${stored.length} selected preset${stored.length === 1 ? "" : "s"}?`)) return;
    setPending(true);
    let completed = 0;
    try {
      const fresh = await gateway.listPresets(false, list.setlistKey);
      for (const selected of stored) {
        const entry = fresh.presets.find((candidate) => candidate.position === selected.position);
        if (!entry || entry.occupied === false || entry.name !== selected.name) throw new Error(`${selected.location} changed or disappeared. The remaining presets were not deleted.`);
      }
      for (const selected of stored) {
        await gateway.deletePreset(list.setlistKey, selected.position, selected.name);
        completed += 1;
      }
      const refreshed = await loadDirectory(false, list.setlistKey, true);
      const verified = refreshed && stored.every((selected) => {
        const after = refreshed.presets.find((candidate) => candidate.position === selected.position);
        return !after || after.occupied === false || after.name === "Unsaved";
      });
      if (!verified) throw new Error("The delete completed, but at least one selected slot remains occupied in the catalog readback.");
      notice(`${stored.length} presets deleted and verified.`);
    } catch (error) {
      try { await loadDirectory(false, list.setlistKey, true); } catch { /* Preserve the operation error. */ }
      fail(completed ? new Error(`${completed} of ${stored.length} presets were deleted before the operation stopped. The Directory was refreshed; inspect it before retrying. ${error instanceof Error ? error.message : String(error)}`) : error);
    } finally { setPending(false); }
  }, [fail, gateway, loadDirectory, notice, presetList, prompts, setPending, unavailable]);

  const arrangePreset = useCallback(async (source: PresetEntry, destination: PresetEntry) => {
    const list = presetList;
    if (!list || unavailable("arranging presets")) return;
    if (source.position === destination.position) { notice("Choose a different destination slot."); return; }
    if (source.occupied === false || source.name === "Unsaved") { notice("An empty slot cannot be moved or swapped."); return; }
    setPending(true);
    try {
      const fresh = await gateway.listPresets(false, list.setlistKey);
      const from = fresh.presets.find((candidate) => candidate.position === source.position);
      const to = fresh.presets.find((candidate) => candidate.position === destination.position);
      if (!from || from.name !== source.name || from.occupied === false || !to) throw new Error("The selected preset slots changed. Refresh the Directory before arranging them.");
      const occupied = to.occupied !== false && to.name !== "Unsaved";
      const effect = occupied ? `swap ${from.location} · ${from.name} with ${to.location} · ${to.name}` : `move ${from.location} · ${from.name} to empty slot ${to.location}`;
      if (!await prompts.confirm(`This is permanent: ${effect}?`)) return;
      await gateway.movePreset(list.setlistKey, from.position, from.name, to.position);
      const refreshed = await loadDirectory(false, list.setlistKey, true);
      if (!refreshed) throw new Error("The arrangement was sent, but the updated catalog could not be verified.");
      const moved = refreshed.presets.find((candidate) => candidate.position === to.position);
      const sourceAfter = refreshed.presets.find((candidate) => candidate.position === from.position);
      if (moved?.name !== from.name || (occupied && sourceAfter?.name !== to.name) || (!occupied && sourceAfter?.occupied !== false && sourceAfter?.name !== "Unsaved")) {
        throw new Error("The device accepted the arrangement, but catalog readback did not match. Inspect the Directory before retrying.");
      }
      notice(occupied ? "Presets swapped and verified." : "Preset moved and verified.");
    } catch (error) {
      try { await loadDirectory(false, list.setlistKey, true); } catch { /* Preserve the operation error. */ }
      fail(error);
    } finally { setPending(false); }
  }, [fail, gateway, loadDirectory, notice, pending, presetList, prompts, setPending, unavailable]);

  const refresh = useCallback(async () => {
    if (unavailable("refreshing device state")) return;
    setPending(true);
    notice("Refreshing complete device state…");
    try {
      const current = await gateway.currentSnapshot();
      commitPreset(current);
      notice("Live preset state refreshed.");
    } catch (error) {
      fail(error);
    } finally {
      setPending(false);
    }
  }, [commitPreset, fail, gateway, notice, setPending, unavailable]);

  const createSetlist = useCallback(async () => {
    if (unavailable("creating a setlist")) return;
    const entered = await prompts.prompt("New Setlist name", "");
    if (entered === null) return;
    const name = entered.trim();
    if (!name) { notice("Enter a name for the new setlist."); return; }
    setPending(true);
    notice(`Creating setlist ${name}…`);
    try {
      const result = await gateway.createSetlist(name);
      if (result.snapshot) commitPreset(result.snapshot);
      const folders = await gateway.listPresetFolders(false);
      foldersLoaded.current = true;
      setPresetList((current) => current ? { ...current, folders: folders.folders } : current);
      const created = folders.folders.find((folder) => folder.name.toLocaleLowerCase() === name.toLocaleLowerCase());
      if (!created) throw new Error(`Setlist ${name} was created, but it was not present in the refreshed folder list.`);
      const refreshed = await loadDirectory(false, created.key);
      if (!refreshed) throw new Error(`Setlist ${name} was created, but its preset list could not be reloaded.`);
      notice(result.detail ?? `Setlist ${name} created.`);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [commitPreset, fail, gateway, loadDirectory, notice, prompts, setPending, unavailable]);

  const deletePresetEntry = useCallback(async (entry: PresetEntry) => {
    const list = presetList;
    if (!list || unavailable("deleting a preset")) return;
    const folder = list.folders.find((candidate) => candidate.key === list.setlistKey);
    if (folder?.isFactory) { notice("Factory presets cannot be deleted."); return; }
    if (entry.occupied === false || entry.name === "Unsaved") { notice("An empty preset slot cannot be deleted."); return; }
    if (!await prompts.confirm(`Delete ${entry.location} · ${entry.name} from ${list.setlistName}? This cannot be undone.`)) return;
    setPending(true);
    notice(`Deleting ${entry.location} · ${entry.name}…`);
    try {
      const result = await gateway.deletePreset(list.setlistKey, entry.position, entry.name);
      if (result.snapshot) commitPreset(result.snapshot);
      const refreshed = await loadDirectory(false, list.setlistKey);
      if (!refreshed) throw new Error(`Preset deletion was accepted, but the directory could not be authoritatively reloaded.`);
      notice(result.detail ?? `${entry.location} · ${entry.name} deleted.`);
    } catch (error) { fail(error); }
    finally { setPending(false); }
  }, [commitPreset, fail, gateway, loadDirectory, notice, pending, presetList, prompts, setPending, unavailable]);

  // A gateway call in flight and a device still streaming are both "not here
  // yet" from the user's side, and either one must keep the waiting indicator
  // up. They are now one signal: the broker blocks a catalog read until this
  // USB session's directory publication has reached the requested setlist, so
  // an outstanding call is exactly "the pull has not arrived yet" - including
  // the background warm-up that runs before the browser is ever opened.
  const catalogPending = presetListLoading || presetFoldersLoading;

  const directoryProps: PresetDirectoryState = {
    interactionSessionKey: gateway,
    open: directoryOpen,
    list: presetList,
    loading: presetListLoading || presetFoldersLoading,
    pending: catalogPending,
    disabled: pending,
    onClose: () => setDirectoryOpen(false),
    onRefresh: () => void openDirectory(true),
    onRecall: (entry) => void recall(entry),
    onSelectSetlist: (setlistKey) => void loadDirectory(false, setlistKey),
    onNewSetlist: () => void createSetlist(),
    onPresetAction: (action, entry) => {
      if (action === "delete") void deletePresetEntry(entry);
      else void directoryPresetAction(action, entry);
    },
    onPresetBatchAction: (action, entries) => void directoryPresetBatchAction(action, entries),
    onArrangePreset: (source, destination) => void arrangePreset(source, destination),
    library,
    onLoadLibrary: loadLibrary,
    setlistDates,
    onLoadSetlistDates: loadSetlistDates,
    cloudSignedIn
  };

  return {
    presetList,
    directoryOpen,
    directoryLoading: presetListLoading || presetFoldersLoading,
    catalogPending,
    saveOpen,
    saveName,
    clipboard,
    openDirectory,
    closeDirectory: () => setDirectoryOpen(false),
    followCloudSignedIn: setCloudSignedIn,
    loadDirectory,
    recall,
    recallLocation,
    reload,
    openSave,
    closeSave,
    setSaveName,
    save,
    saveCurrentUnsaved,
    copy,
    paste,
    rename,
    createSetlist,
    refresh,
    clearClipboard: () => setClipboard(undefined),
    resetCache: () => {
      loadSequence.current += 1;
      foldersLoaded.current = false;
      // A reset discards a previous session's publication, so the next one must
      // be allowed to warm the browser again.
      catalogWarmed.current = false;
      setPresetList(undefined);
      setLibrary(EMPTY_DIRECTORY_LIBRARY);
      setCloudSignedIn(undefined);
      requestedSetlistDates.current = new Set();
      setSetlistDates(new Map());
    },
    directoryProps,
    saveProps: {
      open: saveOpen,
      name: saveName,
      disabled: pending,
      onNameChange: setSaveName,
      onSave: () => void save(),
      onCancel: () => {
        closeSave();
        notice("Preset save cancelled.");
      }
    }
  };
}
