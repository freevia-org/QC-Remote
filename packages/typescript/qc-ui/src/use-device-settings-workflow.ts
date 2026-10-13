import { useCallback, useEffect, useRef, useState } from "react";
import type { DeviceActionResult, DeviceIdentity, GeneralIntegerSetting, GeneralSettings, GeneralToggleSetting } from "@qc-remote/client";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import { observedGlobalBypassUpdate } from "./global-bypass-update.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

/** Section of the device's own Settings dialog - the header/left-nav pill the
 * unit itself draws (coros-screen-fixtures.tsx's CorOsOfficialSettings).
 * Account, System, Device, and Support are peers. MIDI is not a fifth peer:
 * SETTINGS_DEVICE_ROWS (that fixture file's own
 * shared row table) lists "MIDI" as the eighth Device row, and its own
 * `view === "settings-midi"` branch still renders the header pill labelled
 * "Device" - MIDI is a scrolled state of Device's row nav, not its own
 * category. This hook and CorOsDeviceSettings both follow that, not the
 * literal 4-way `view` union CorOsOfficialSettings's TypeScript happens to
 * spell (an artifact of how the fixture's own two components split up, not a
 * separate thing the device draws). */
export type DeviceSettingsSection = "account" | "system" | "device" | "support";

export const DEVICE_SETTINGS_ACCOUNT_ROWS = ["user", "backup"] as const;
export type DeviceSettingsAccountRow = typeof DEVICE_SETTINGS_ACCOUNT_ROWS[number];

export const DEVICE_SETTINGS_SYSTEM_ROWS = ["wifi", "updates", "brightness", "power", "volume", "storage", "factory"] as const;
export type DeviceSettingsSystemRow = typeof DEVICE_SETTINGS_SYSTEM_ROWS[number];

/** SETTINGS_DEVICE_ROWS in coros-screen-fixtures.tsx, verbatim order. */
export const DEVICE_SETTINGS_DEVICE_ROWS = ["bypass", "scene-bypass", "stomp-bypass", "hold", "swap", "gig-access", "latency", "midi"] as const;
export type DeviceSettingsDeviceRow = typeof DEVICE_SETTINGS_DEVICE_ROWS[number];

export const DEVICE_SETTINGS_SUPPORT_ROWS = ["about", "info", "report", "diagnostics", "licenses"] as const;
export type DeviceSettingsSupportRow = typeof DEVICE_SETTINGS_SUPPORT_ROWS[number];

/** `holdTiming`'s six wire steps (`GeneralIntegerSetting`), and the
 * `holdTimingMs` each one reads back as: 500ms to 1000ms in six steps. The
 * millisecond figures are what the unit showed for indices 0, 3 and 5 (observed
 * on a CorOS 4.1.0 unit, 2026-10-06). */
export const HOLD_TIMING_STEPS_MS = [500, 600, 700, 800, 900, 1000] as const;

/** `midiClockOut`'s wire ordinal for `setGeneralInteger`, against the string
 * `generalSettings()` reads it back as: off/midiDinOnly/usbMidiOnly/
 * bothUsbAndDinMidi, 0 through 3. */
export const MIDI_CLOCK_OUT_VALUES = ["off", "midiDinOnly", "usbMidiOnly", "bothUsbAndDinMidi"] as const;
export type MidiClockOutValue = typeof MIDI_CLOCK_OUT_VALUES[number];

export type DeviceSettingsWorkflowOptions = DeviceGatewayWorkflowOptions & {
  /** Stable identity of the selected device/session, including virtual sources. */
  sessionKey?: unknown;
  /** Immediate boundary guard while native events are ahead of React's render. */
  captureSessionCurrent?(): () => boolean;
};

/**
 * The device's own Settings screen (Account/System/Device/Support, with MIDI
 * inside Device), reached from the Grid's own context menu (coros-ui.ts
 * GRID_CONTEXT_MENU, action "settings") - never the generic app-preferences
 * window `appWindows.open("settings")` opens (App.tsx's MenuBar "Settings…"
 * item, untouched by this hook). `GeneralSettings` is device state, not
 * preset state: it has no place on `PresetSnapshot` and no undo path either
 * (`gateway.undo`/`redo` operate on the preset's command history, which
 * device settings were never part of), so unlike every sibling workflow in
 * this file's family this one carries no `snapshot`/`reconcile`/
 * `recordHistory` - there is nothing of either shape to reconcile against or
 * record.
 *
 * Every setter re-reads `generalSettings()` after a successful write instead
 * of trusting an optimistic merge or a write's own `readback`, which keeps
 * this hook honest on the virtual device: `setMasterVolumeAssignment`/
 * `setSceneBypassBehavior` are real there but `generalSettings()` itself is
 * not (virtual-gateway.ts's own "not offered by a virtual device" list), so
 * the refresh is allowed to fail silently after a write that itself
 * succeeded - the write's own `result.detail` notice already told the user
 * what happened.
 */
export function useDeviceSettingsWorkflow(options: DeviceSettingsWorkflowOptions) {
  const { gateway, pending, notice, fail } = options;
  const ownerRef = useRef<{ key: unknown; gateway: typeof gateway; connected: boolean; current: () => boolean } | undefined>(undefined);
  if (!ownerRef.current || ownerRef.current.key !== options.sessionKey || ownerRef.current.gateway !== gateway
      || ownerRef.current.connected !== options.connected) {
    ownerRef.current = { key: options.sessionKey, gateway, connected: options.connected,
      current: options.captureSessionCurrent?.() ?? (() => true) };
  }
  const owner = ownerRef.current;
  const mounted = useRef(true);
  const pendingOwner = useRef<typeof owner | undefined>(undefined);
  const latestSetPending = useRef(options.setPending);
  latestSetPending.current = options.setPending;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (pendingOwner.current) { pendingOwner.current = undefined; latestSetPending.current(false); }
    };
  }, []);
  useEffect(() => {
    if (pendingOwner.current && pendingOwner.current !== owner) {
      pendingOwner.current = undefined;
      latestSetPending.current(false);
    }
  }, [owner]);
  const current = useCallback(() => mounted.current && ownerRef.current === owner && owner.connected && owner.current(), [owner]);
  const guarded = useGuardedMutation({ ...options,
    setPending: (value) => {
      if (value && current()) { pendingOwner.current = owner; options.setPending(true); }
      else if (!value && pendingOwner.current === owner && current()) { pendingOwner.current = undefined; options.setPending(false); }
    },
    notice: (message) => { if (current()) notice(message); },
    fail: (error) => { if (current()) fail(error); }
  }, "changing device settings");
  const [opening, setOpening] = useState<{ owner: typeof owner; open: boolean; returnFocusTarget?: "grid-menu" }>({ owner, open: false });
  const open = opening.owner === owner && opening.open;
  const returnFocusTarget = opening.owner === owner ? opening.returnFocusTarget : undefined;
  const [loadingRecord, setLoadingRecord] = useState<{ owner: typeof owner; value: boolean }>();
  const loading = loadingRecord?.owner === owner ? loadingRecord.value : false;
  const setLoading = useCallback((value: boolean) => { if (current()) setLoadingRecord({ owner, value }); }, [current, owner]);
  const [settingsRecord, setSettingsRecord] = useState<{ owner: typeof owner; value: GeneralSettings }>();
  const [identityRecord, setIdentityRecord] = useState<{ owner: typeof owner; value: DeviceIdentity }>();
  const settings = settingsRecord?.owner === owner && current() ? settingsRecord.value : undefined;
  const identity = identityRecord?.owner === owner && current() ? identityRecord.value : undefined;
  const setSettings = useCallback((value: GeneralSettings) => { if (current()) setSettingsRecord({ owner, value }); }, [current, owner]);
  const setIdentity = useCallback((value: DeviceIdentity) => { if (current()) setIdentityRecord({ owner, value }); }, [current, owner]);
  // The unit opens Settings on System with its first row, Connection, selected
  // (walked on the device from the Grid menu, twice: the Connection pane with
  // its network list). corpus settings-system.png shows Brightness selected
  // only because that capture tapped the row first. Which section and row the
  // unit shows on a LATER open is not established by any frame, so the choice
  // made here is simply kept for as long as this hook lives.
  const [section, setSection] = useState<DeviceSettingsSection>("system");
  const [accountRow, setAccountRow] = useState<DeviceSettingsAccountRow>("user");
  const [systemRow, setSystemRow] = useState<DeviceSettingsSystemRow>("wifi");
  const [deviceRow, setDeviceRow] = useState<DeviceSettingsDeviceRow>("bypass");
  const [supportRow, setSupportRow] = useState<DeviceSettingsSupportRow>("about");
  // `generalSettings()` projects no `midiThru` field at all - grep
  // packages/typescript/qc-client/src/generated-payloads.ts's `GeneralSettings`
  // interface - so unlike every other MIDI row this one has no authoritative
  // read path to refresh from. Tracked locally, optimistically, from this
  // hook's own successful writes only; `undefined` until the first one.
  const [midiThruRecord, setMidiThruRecord] = useState<{ owner: typeof owner; value: boolean }>();
  const midiThru = midiThruRecord?.owner === owner && current() ? midiThruRecord.value : undefined;
  const setMidiThruState = useCallback((value: boolean) => { if (current()) setMidiThruRecord({ owner, value }); }, [current, owner]);

  const refresh = useCallback(async () => {
    if (!current()) return;
    try {
      setSettings(await gateway.generalSettings());
    } catch {
      // Reading is unavailable on the virtual device and possibly other
      // gateways; leave whatever was last observed (every GeneralSettings
      // field is optional, so the screen still renders, just without that
      // row's live value) rather than surfacing a second error on top of a
      // write that may have just succeeded.
    }
  }, [gateway, current, setSettings]);

  const openSettings = useCallback((returnFocusTarget?: "grid-menu") => {
    if (!current()) return;
    setOpening({ owner, open: true, returnFocusTarget });
    setLoading(true);
    void Promise.allSettled([
      gateway.generalSettings().then(setSettings),
      gateway.identity().then(setIdentity)
    ]).then(([settingsResult, identityResult]) => {
      if (current() && settingsResult.status === "rejected" && identityResult.status === "rejected") fail(settingsResult.reason);
    }).finally(() => { if (current()) setLoading(false); });
  }, [fail, gateway, current, setIdentity, setSettings, owner, setLoading]);

  const close = useCallback(() => setOpening({ owner, open: false }), [owner]);

  const commit = useCallback(async (labelText: string, action: () => Promise<DeviceActionResult>) => {
    if (!current()) return;
    await guarded.run({
      progress: `${labelText}…`,
      write: action,
      settle: async (result) => {
        if (!current()) return;
        notice(result.detail);
        await refresh();
      }
    });
  }, [guarded.run, notice, refresh, current]);

  const setToggle = useCallback((setting: GeneralToggleSetting, label: string, enabled: boolean) =>
    commit(`${enabled ? "Enabling" : "Disabling"} ${label}`, () => gateway.setGeneralToggle(setting, enabled)), [commit, gateway]);

  const setInteger = useCallback((setting: GeneralIntegerSetting, label: string, value: number) =>
    commit(`Setting ${label}`, () => gateway.setGeneralInteger(setting, value)), [commit, gateway]);

  // -- Device ---------------------------------------------------------------
  const setGlobalBypass = useCallback(async (category: "cab" | "ir", row: 1 | 2 | 3 | 4, bypassed: boolean) => {
    if (!current()) return;
    const update = observedGlobalBypassUpdate(settings, category, row, bypassed);
    if (!update) {
      notice("Global Bypass rows have not been read completely. Reopen Settings before changing them.");
      return;
    }
    const { cab, ir } = update;
    await commit(`Updating Global Bypass row ${row}`, () => gateway.setGlobalBypass(cab, ir));
  }, [commit, gateway, notice, settings, current]);

  const setSceneBypassBehavior = useCallback((behavior: "alwaysOverwrite" | "nonstompOverwrite" | "neverOverwrite") =>
    commit("Setting Scene Bypass Behavior", () => gateway.setSceneBypassBehavior(behavior)), [commit, gateway]);

  const setStompModeAutoAssign = useCallback((enabled: boolean) => setToggle("stompModeAutoAssign", "Stomp Mode Bypass", enabled), [setToggle]);
  const setHoldTimingIndex = useCallback((index: number) => setInteger("holdTiming", "Hold Timing", index), [setInteger]);
  const setSwapTempoTunerAccess = useCallback((enabled: boolean) => setToggle("swapTempoTunerAccess", "Swap Tempo and Tuner", enabled), [setToggle]);
  const setGigViewStompAccess = useCallback((enabled: boolean) => setToggle("gigViewStompAccess", "Gig View Access", enabled), [setToggle]);
  const setDynamicDelayCompensation = useCallback((enabled: boolean) => setToggle("dynamicDelayCompensation", "Latency Compensation", enabled), [setToggle]);

  // -- MIDI (the Device row's own eighth entry) ------------------------------
  const setMidiChannel = useCallback((channel: number) => setInteger("midiChannel", "MIDI Channel", channel), [setInteger]);
  const setMidiThru = useCallback(async (enabled: boolean) => {
    if (!current()) return;
    await guarded.run({
      progress: `${enabled ? "Enabling" : "Disabling"} MIDI Thru…`,
      write: () => gateway.setMidiThru(enabled),
      settle: (result) => {
        if (!current()) return;
        notice(result.detail);
        setMidiThruState(enabled);
      }
    });
  }, [gateway, guarded.run, notice, current, setMidiThruState]);
  const setMidiOverUsb = useCallback((enabled: boolean) => setToggle("midiOverUsb", "MIDI Over USB", enabled), [setToggle]);
  const setIgnoreDuplicatePc = useCallback((enabled: boolean) => setToggle("ignoreDuplicatePc", "Ignore Duplicate PC", enabled), [setToggle]);
  const setMidiClockOut = useCallback((value: MidiClockOutValue) => setInteger("midiClockOut", "MIDI Clock Out", MIDI_CLOCK_OUT_VALUES.indexOf(value)), [setInteger]);
  const setMidiClockIn = useCallback((enabled: boolean) => setToggle("midiClockIn", "MIDI Clock In", enabled), [setToggle]);

  // -- System -----------------------------------------------------------------
  const setScreenBrightness = useCallback((value: number) => setInteger("screenBrightness", "Screen brightness", value), [setInteger]);
  const setLedBrightness = useCallback((value: number) => setInteger("ledBrightness", "LED brightness", value), [setInteger]);
  const setDimmedLedBrightness = useCallback((value: number) => setInteger("dimmedLedBrightness", "Dimmed LED brightness", value), [setInteger]);
  // Which modes dim their LEDs: the three marks along the bottom of Brightness.
  const setModeDimmed = useCallback((mode: "PRESET" | "SCENE" | "STOMP", enabled: boolean) =>
    mode === "PRESET" ? setToggle("presetDimmed", "Preset mode dimmed LEDs", enabled)
      : mode === "SCENE" ? setToggle("sceneDimmed", "Scene mode dimmed LEDs", enabled)
      : setToggle("stompDimmed", "Stomp mode dimmed LEDs", enabled), [setToggle]);
  const setMasterVolumeAssignment = useCallback((out12: boolean, out34: boolean, send12: boolean, headphones: boolean) =>
    commit("Setting Master Volume Knob assignment", () => gateway.setMasterVolumeAssignment(out12, out34, send12, headphones)), [commit, gateway]);

  return {
    open, returnFocusTarget, openSettings, close, loading, refresh, announce: notice,
    settings, identity, midiThru,
    section, setSection,
    accountRow, setAccountRow,
    systemRow, setSystemRow,
    deviceRow, setDeviceRow,
    supportRow, setSupportRow,
    pending,
    setGlobalBypass, setSceneBypassBehavior, setStompModeAutoAssign, setHoldTimingIndex, setSwapTempoTunerAccess, setGigViewStompAccess, setDynamicDelayCompensation,
    setMidiChannel, setMidiThru, setMidiOverUsb, setIgnoreDuplicatePc, setMidiClockOut, setMidiClockIn,
    setScreenBrightness, setLedBrightness, setDimmedLedBrightness, setModeDimmed, setMasterVolumeAssignment
  };
}

export type DeviceSettingsWorkflow = ReturnType<typeof useDeviceSettingsWorkflow>;
