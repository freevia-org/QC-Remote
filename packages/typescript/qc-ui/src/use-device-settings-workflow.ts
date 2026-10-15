import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import { isCorOsVersion, type ObservedCorOsVersion } from "./cor-os-version.ts";
import type { QcSettingsUnitSelection } from "./settings-unit-selection.ts";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { DeviceActionResult, DeviceIdentity, GatewayTransport, GeneralIntegerSetting, GeneralSettings, GeneralToggleSetting } from "@qc-remote/client";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import { captureGatewaySession } from "./gateway-session.ts";
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

export type DeviceSettingsWorkflowOptions = DeviceGatewayWorkflowOptions & { ioSettingsError?: string };

/** A bypass write replaces four rows, so every unedited row must be observed,
 * never invented. The other family is deliberately absent from the write. */
export function globalBypassUpdate(settings: GeneralSettings, category: "cab" | "ir", row: 1 | 2 | 3 | 4, bypassed: boolean) {
  const family = category === "cab" ? settings.globalBypassCab : settings.globalBypassIr;
  const values = [family?.row1, family?.row2, family?.row3, family?.row4];
  if (!values.every((value) => typeof value === "boolean")) throw new Error("Global Bypass state is unavailable. Refresh device settings before changing a row.");
  const rows = values as [boolean, boolean, boolean, boolean];
  rows[row - 1] = bypassed;
  return { cab: category === "cab" ? rows : null, ir: category === "ir" ? rows : null };
}

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
  const workflowSession = useDeviceWorkflowSession(options);
  const stateOwner = useRef(workflowSession.key);
  const currentState = stateOwner.current === workflowSession.key && workflowSession.current();
  useEffect(() => { stateOwner.current = workflowSession.key; }, [workflowSession.key]);
  const { gateway, lifetime, pending, notice, fail } = workflowSession.options;
  const guarded = useGuardedMutation(workflowSession.options, "changing device settings");
  const [opening, setOpening] = useState<{ open: boolean; returnFocusTarget?: "grid-menu" }>({ open: false });
  const { open, returnFocusTarget } = opening;
  const [loading, setLoading] = useState(false);
  const [settings, setSettings] = useState<GeneralSettings>();
  const [identity, setIdentityState] = useState<DeviceIdentity>();
  const [identityReadAt, setIdentityReadAt] = useState<number>();
  const setIdentity = useCallback((value: DeviceIdentity | undefined) => { setIdentityState(value); setIdentityReadAt(value ? Date.now() : undefined); }, []);
  const [screenVersionRecord, setScreenVersionRecord] = useState<ObservedCorOsVersion>();
  useEffect(() => setScreenVersionRecord(undefined), [workflowSession.key]);
  const noteCorOsVersionFromScreen = useCallback((value: string) => {
    if (!workflowSession.current() || !isCorOsVersion(value)) return;
    const version = value.trim(), at = Date.now();
    setScreenVersionRecord((previous) => previous?.value === version && at - previous.at < 5000 ? previous : { value: version, at });
  }, [workflowSession.current]);
  const screenCorOsVersion = currentState ? screenVersionRecord : undefined;
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
  const followUnitSelection = useCallback((selection: QcSettingsUnitSelection) => {
    if (!workflowSession.current()) return;
    setSection(selection.section);
    if (selection.section === "account") setAccountRow(selection.row);
    else if (selection.section === "system") setSystemRow(selection.row);
    else if (selection.section === "device") setDeviceRow(selection.row);
    else setSupportRow(selection.row);
  }, [workflowSession.current]);
  // `generalSettings()` projects no `midiThru` field at all - grep
  // packages/typescript/qc-client/src/generated-payloads.ts's `GeneralSettings`
  // interface - so unlike every other MIDI row this one has no authoritative
  // read path to refresh from. Tracked locally, optimistically, from this
  // hook's own successful writes only; `undefined` until the first one.
  const [midiThru, setMidiThruState] = useState<boolean>();
  const ioFailure = useRef(options.ioSettingsError);
  const ioGeneration = useRef(0);
  if (ioFailure.current !== options.ioSettingsError) { ioGeneration.current++; ioFailure.current = options.ioSettingsError; }
  useEffect(() => { if (options.ioSettingsError) setMidiThruState(undefined); }, [options.ioSettingsError]);
  const latestGateway = useRef(gateway);
  latestGateway.current = gateway;
  const settingsRead = useRef(0);
  const openingRead = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  // Callbacks belong to the render's device session, not whichever session
  // happens to be current when a retained event finally invokes them.
  const session = lifetime?.checkpoint().session;
  const origin = useMemo(() => lifetime?.checkpoint(), [lifetime, session]);
  const [, refreshSessionRender] = useState(0);
  const captureSession = useCallback((valid: () => boolean) => {
    return captureGatewaySession(gateway, () => mounted.current && valid() && (!origin || lifetime!.currentSession(origin)));
  }, [gateway, lifetime, origin]);
  useEffect(() => {
    const reset = () => {
      settingsRead.current += 1; openingRead.current += 1;
      setSettings(undefined); setIdentity(undefined); setMidiThruState(undefined); setLoading(false);
      // A session can reset while every optional read is already undefined.
      // Still recreate fresh callbacks rather than leave only invalid ones.
      refreshSessionRender((revision) => revision + 1);
    };
    reset();
    let checkpoint = lifetime?.checkpoint();
    const unsubscribe = lifetime?.subscribe(() => {
      if (checkpoint && !lifetime.currentSession(checkpoint)) { checkpoint = lifetime.checkpoint(); reset(); }
    });
    return () => { unsubscribe?.(); settingsRead.current += 1; openingRead.current += 1; };
  }, [gateway, lifetime]);

  const refresh = useCallback(async () => {
    let read = settingsRead.current;
    const session = captureSession(() => latestGateway.current === gateway && settingsRead.current === read);
    // Refuse retained callbacks before they can supersede a current read.
    if (!session.current()) return;
    read = ++settingsRead.current;
    try {
      const observed = await session.gateway.generalSettings();
      if (session.current()) setSettings(observed);
    } catch {
      // Reading is unavailable on the virtual device and possibly other
      // gateways; leave whatever was last observed (every GeneralSettings
      // field is optional, so the screen still renders, just without that
      // row's live value) rather than surfacing a second error on top of a
      // write that may have just succeeded.
    }
  }, [gateway, captureSession]);

  const openSettings = useCallback((returnFocusTarget?: "grid-menu") => {
    let opening = openingRead.current;
    const session = captureSession(() => latestGateway.current === gateway && openingRead.current === opening);
    // Loading and read generations belong only to an admitted session.
    if (!session.current()) return;
    const read = ++settingsRead.current;
    opening = ++openingRead.current;
    setOpening({ open: true, returnFocusTarget });
    setLoading(true);
    void Promise.allSettled([
      session.gateway.generalSettings().then((value) => { if (session.current() && read === settingsRead.current) setSettings(value); }),
      session.gateway.identity().then((value) => { if (session.current()) setIdentity(value); })
    ]).then(([settingsResult, identityResult]) => {
      if (session.current() && settingsResult.status === "rejected" && identityResult.status === "rejected") fail(settingsResult.reason);
    }).finally(() => { if (session.current()) setLoading(false); });
  }, [fail, gateway, captureSession]);

  const close = useCallback(() => {
    openingRead.current += 1;
    settingsRead.current += 1;
    setLoading(false);
    setOpening({ open: false });
  }, []);

  const commit = useCallback(async (labelText: string, action: (gateway: GatewayTransport) => Promise<DeviceActionResult>) => {
    const session = captureSession(() => latestGateway.current === gateway);
    await guarded.run({
      current: session.current,
      progress: `${labelText}…`,
      write: () => action(session.gateway),
      settle: async (result) => {
        notice(result.detail);
        const read = ++settingsRead.current;
        try {
          const observed = await session.gateway.generalSettings();
          if (session.current() && latestGateway.current === gateway && settingsRead.current === read) setSettings(observed);
        } catch { /* Write outcome stands. */ }
      }
    });
  }, [gateway, captureSession, guarded.run, notice]);

  const setToggle = useCallback((setting: GeneralToggleSetting, label: string, enabled: boolean) =>
    commit(`${enabled ? "Enabling" : "Disabling"} ${label}`, (gateway) => gateway.setGeneralToggle(setting, enabled)), [commit]);

  const setInteger = useCallback((setting: GeneralIntegerSetting, label: string, value: number) =>
    commit(`Setting ${label}`, (gateway) => gateway.setGeneralInteger(setting, value)), [commit]);

  // -- Device ---------------------------------------------------------------
  const setGlobalBypass = useCallback(async (category: "cab" | "ir", row: 1 | 2 | 3 | 4, bypassed: boolean) => {
    await commit(`Updating Global Bypass row ${row}`, async (gateway) => {
      const { cab, ir } = globalBypassUpdate(await gateway.generalSettings(), category, row, bypassed);
      return gateway.setGlobalBypass(cab, ir);
    });
  }, [commit]);

  const setSceneBypassBehavior = useCallback((behavior: "alwaysOverwrite" | "nonstompOverwrite" | "neverOverwrite") =>
    commit("Setting Scene Bypass Behavior", (gateway) => gateway.setSceneBypassBehavior(behavior)), [commit]);

  const setStompModeAutoAssign = useCallback((enabled: boolean) => setToggle("stompModeAutoAssign", "Stomp Mode Bypass", enabled), [setToggle]);
  const setHoldTimingIndex = useCallback((index: number) => setInteger("holdTiming", "Hold Timing", index), [setInteger]);
  const setSwapTempoTunerAccess = useCallback((enabled: boolean) => setToggle("swapTempoTunerAccess", "Swap Tempo and Tuner", enabled), [setToggle]);
  const setGigViewStompAccess = useCallback((enabled: boolean) => setToggle("gigViewStompAccess", "Gig View Access", enabled), [setToggle]);
  const setDynamicDelayCompensation = useCallback((enabled: boolean) => setToggle("dynamicDelayCompensation", "Latency Compensation", enabled), [setToggle]);

  // -- MIDI (the Device row's own eighth entry) ------------------------------
  const setMidiChannel = useCallback((channel: number) => setInteger("midiChannel", "MIDI Channel", channel), [setInteger]);
  const setMidiThru = useCallback(async (enabled: boolean) => {
    if (ioFailure.current) { notice(ioFailure.current); return; }
    const generation = ioGeneration.current;
    const session = captureSession(() => latestGateway.current === gateway && generation === ioGeneration.current);
    await guarded.run({
      current: session.current,
      progress: `${enabled ? "Enabling" : "Disabling"} MIDI Thru…`,
      write: () => session.gateway.setMidiThru(enabled),
      settle: (result) => {
        notice(result.detail);
        setMidiThruState(enabled);
      }
    });
  }, [gateway, captureSession, guarded.run, notice]);
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
    commit("Setting Master Volume Knob assignment", (gateway) => gateway.setMasterVolumeAssignment(out12, out34, send12, headphones)), [commit]);
  const setMasterVolumeOutput = useCallback((field: "out12" | "out34" | "send12" | "headphones", enabled: boolean) =>
    commit("Setting Master Volume Knob assignment", async (gateway) => {
      const assignment = (await gateway.generalSettings()).masterVolumeAssignment;
      if (!assignment || ![assignment.out12, assignment.out34, assignment.send12, assignment.headphones].every((value) => typeof value === "boolean")) {
        throw new Error("Master Volume assignment is unavailable. Refresh device settings before changing an output.");
      }
      const next = { ...assignment };
      next[field] = enabled;
      const { out12, out34, send12, headphones } = next;
      return gateway.setMasterVolumeAssignment(out12, out34, send12, headphones);
    }), [commit]);

  return {
    open: currentState && open, returnFocusTarget: currentState ? returnFocusTarget : undefined, openSettings, close, loading: currentState && loading, refresh, announce: notice,
    settings: currentState ? settings : undefined, identity: currentState ? identity : undefined, midiThru: !currentState || options.ioSettingsError ? undefined : midiThru,
    identityReadAt: currentState ? identityReadAt : undefined, screenCorOsVersion, noteCorOsVersionFromScreen,
    section, setSection, followUnitSelection,
    accountRow, setAccountRow,
    systemRow, setSystemRow,
    deviceRow, setDeviceRow,
    supportRow, setSupportRow,
    pending,
    setGlobalBypass, setSceneBypassBehavior, setStompModeAutoAssign, setHoldTimingIndex, setSwapTempoTunerAccess, setGigViewStompAccess, setDynamicDelayCompensation,
    setMidiChannel, setMidiThru, setMidiOverUsb, setIgnoreDuplicatePc, setMidiClockOut, setMidiClockIn,
    setScreenBrightness, setLedBrightness, setDimmedLedBrightness, setModeDimmed, setMasterVolumeAssignment, setMasterVolumeOutput
  };
}

export type DeviceSettingsWorkflow = ReturnType<typeof useDeviceSettingsWorkflow>;
