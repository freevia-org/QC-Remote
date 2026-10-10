import { useCallback, useState } from "react";
import { QC_MAXIMUM_TEMPO_BPM, QC_MINIMUM_TEMPO_BPM } from "@qc-remote/client";
import type { DeviceActionResult, TempoClockState, TempoSettings } from "@qc-remote/client";
import { useGuardedMutation } from "./use-guarded-mutation.ts";
import type { DeviceGatewayWorkflowOptions } from "./workflow-options";

/** TempoSettings's own enum members (generated-payloads.ts), verbatim - the
 * T/SIGNATURE/SUBDIVISIONS/SOUND/ROUTING picker rows' real option lists. */
export const TEMPO_TIME_SIGNATURES = ["2/4", "3/4", "4/4", "5/4", "6/4", "7/4", "8/4", "9/4", "10/4", "11/4", "12/4", "13/4", "3/8", "6/8", "9/8", "12/8", "5/8 (3+2)", "5/8 (2+3)", "7/8 (3+2+2)", "7/8 (2+3+2)", "7/8 (2+2+3)"] as const;
export const TEMPO_SUBDIVISIONS = ["1/4", "1/8", "1/8T", "1/16", "1/16T", "1/32"] as const;
export const TEMPO_SOUNDS = ["BLIP", "BLOCK", "COWBELL", "DIGITAL", "DRUM KIT", "SOFT KIT"] as const;
export const TEMPO_ROUTINGS = ["MULTI", "HP", "OUT 1/2", "OUT 3/4", "SEND 1/2"] as const;

export type TempoWorkflowOptions = DeviceGatewayWorkflowOptions;

/**
 * The device's own Tempo & Metronome screen (coros-screen-fixtures.tsx's
 * CorOsTempo), reached from App.tsx's "Tempo…" menu item and the real
 * device's own TEMPO-TEMPO footswitch gesture. `GatewayTransport`'s tempo
 * surface splits into two independent halves:
 *
 *  - `setTempo(bpm, expectedTempo, expectedPresetName, expectedPresetIdentity)`
 *    is preset-scoped (it takes the same preset guard every other preset
 *    write does) and writes `PresetSnapshot.tempo` - the BPM CorOS shows in
 *    PRESET mode. This hook does not touch it: use-continuous-control-
 *    workflow.ts's `queueTempo` already coalesces rapid encoder/drag writes
 *    against it with its own debounce, undo-history and demo-mode handling,
 *    and duplicating that here would fork behaviour that already works. The
 *    Tempo screen keeps calling the existing `queueTempo`/`adjustTempo` for
 *    the PRESET-mode BPM case; this hook supplies everything else.
 *  - `setGlobalTempo(bpm, expectedMode, expectedGlobalBpm)`,
 *    `setTempoMode(mode)` and `setTempoMetronome(ledEnabled, volumeDb,
 *    running, pan, timeSignature, subdivision, sound, routing, beats)` are
 *    all device-wide: none of the three takes a preset guard, matching how
 *    `GeneralSettings`/`useDeviceSettingsWorkflow` (this hook's own closest
 *    precedent) has no place on `PresetSnapshot` and no undo path either.
 *    `TempoSettings.mode` is the switch between the two BPM values CorOS
 *    tracks at once - `bpm` (preset-scoped, written above) and `globalBpm`
 *    (device-wide, written by `setGlobalTempo` here) - so the TEMPO dial
 *    shows/writes `globalBpm` only while `mode === "GLOBAL"`, `bpm`/
 *    `queueTempo` otherwise.
 *
 * Every setter here re-reads `globalTempoSettings()`/`currentTempoClock()`
 * after a successful write, the same "re-read rather than assume" discipline
 * use-device-settings-workflow.ts's own module comment established - and for
 * the same reason: none of these fields live on `PresetSnapshot`, so there is
 * nothing else to reconcile against. Shares its `pending`/`setPending` with
 * every other device-mutation workflow in App.tsx (the same single in-flight-
 * command flag `commandPending` already gates), rather than tracking its own
 * - the same choice useDeviceSettingsWorkflow made.
 *
 * `volumeDb` and `pan` use the wire contract's own ranges (Rust gateway,
 * responses.rs / device/request.rs): volume -60..+9 dB, pan -1..+1 with 0 as
 * the centre ("C"). `volumeDb` is the dB the unit's Tempo screen displays; the
 * gateway converts to and from the raw parameter with the unit's measured
 * (non-linear) taper, commands.rs TEMPO_VOLUME_KNOTS. An earlier revision borrowed a 0..10 pan and a -80..12 dB
 * volume from the Splitter/Mixer and a UI guess, which showed a centred pan
 * as "L50" and would have sent values the gateway rejects.
 */
export function useTempoWorkflow(options: TempoWorkflowOptions) {
  const { gateway, pending, notice } = options;
  const guarded = useGuardedMutation(options, "changing tempo and metronome settings");
  const [opening, setOpening] = useState<{ open: boolean; returnFocusTarget?: "grid-menu" }>({ open: false });
  const { open, returnFocusTarget } = opening;
  const [loading, setLoading] = useState(false);
  const [settings, setSettings] = useState<TempoSettings>();
  const [tempoClock, setTempoClock] = useState<TempoClockState>();

  const refresh = useCallback(async () => {
    // Reading is unavailable on the virtual device and possibly other
    // gateways (virtual-gateway.ts marks globalTempoSettings unavailable) -
    // leave whatever was last observed rather than surfacing a second error
    // on top of a write that may have just succeeded.
    try { setSettings(await gateway.globalTempoSettings()); } catch { /* see above */ }
    try { setTempoClock(await gateway.currentTempoClock()); } catch { /* see above */ }
  }, [gateway]);

  const openTempo = useCallback((returnFocusTarget?: "grid-menu") => {
    setOpening({ open: true, returnFocusTarget });
    setLoading(true);
    void Promise.allSettled([
      gateway.globalTempoSettings().then(setSettings),
      gateway.currentTempoClock().then(setTempoClock)
    ]).finally(() => setLoading(false));
  }, [gateway]);

  const close = useCallback(() => setOpening({ open: false }), []);

  const commit = useCallback(async (labelText: string, action: () => Promise<DeviceActionResult>) => {
    await guarded.run({
      write: action,
      settle: async (result) => {
        notice(result.detail ?? `${labelText}.`);
        await refresh();
      }
    });
  }, [guarded.run, notice, refresh]);

  const setLedEnabled = useCallback((enabled: boolean) =>
    commit(`Tempo LED ${enabled ? "on" : "off"}`, () => gateway.setTempoMetronome(enabled, null, null, null, null, null, null, null, null)), [commit, gateway]);

  // TempoSettings.running names whether the metronome is currently audible -
  // the fixture's own button starts labelled "UNMUTE" (metronome silent,
  // running falsy), so this toggles straight to the requested state rather
  // than inferring a "mute" verb from it.
  const setRunning = useCallback((running: boolean) =>
    commit(running ? "Unmuting the metronome" : "Muting the metronome", () => gateway.setTempoMetronome(null, null, running, null, null, null, null, null, null)), [commit, gateway]);

  const setVolumeDb = useCallback((value: number) =>
    commit(`Metronome Volume ${value.toFixed(1)} dB`, () => gateway.setTempoMetronome(null, value, null, null, null, null, null, null, null)), [commit, gateway]);

  const setPan = useCallback((value: number) =>
    commit("Metronome Pan", () => gateway.setTempoMetronome(null, null, null, value, null, null, null, null, null)), [commit, gateway]);

  const setTimeSignature = useCallback((value: NonNullable<TempoSettings["timeSignature"]>) =>
    commit(`Time Signature ${value}`, () => gateway.setTempoMetronome(null, null, null, null, value, null, null, null, null)), [commit, gateway]);

  const setSubdivision = useCallback((value: NonNullable<TempoSettings["subdivision"]>) =>
    commit(`Subdivisions ${value}`, () => gateway.setTempoMetronome(null, null, null, null, null, value, null, null, null)), [commit, gateway]);

  const setSound = useCallback((value: NonNullable<TempoSettings["sound"]>) =>
    commit(`Metronome Sound ${value}`, () => gateway.setTempoMetronome(null, null, null, null, null, null, value, null, null)), [commit, gateway]);

  // The one metronome field that confirms before writing: ROUTING moves the
  // metronome's audio to a different physical output, the same "changes
  // device-wide signal routing immediately" reasoning use-device-settings-
  // workflow.ts's own setGlobalBypass already confirms for - and the same
  // "Audio may be interrupted" wording use-routing-workflow.ts already uses
  // for a routing write. Every other setter above matches the rest of that
  // same hook's precedent ("every other setter does not [confirm]").
  const setRouting = useCallback(async (value: NonNullable<TempoSettings["routing"]>) => {
    await commit(`Metronome Routing ${value}`, () => gateway.setTempoMetronome(null, null, null, null, null, null, null, value, null));
  }, [commit, gateway]);

  const setMode = useCallback((mode: "PRESET" | "GLOBAL") =>
    commit(`Tempo Mode ${mode === "GLOBAL" ? "Global" : "Preset"}`, () => gateway.setTempoMode(mode)), [commit, gateway]);

  const setGlobalBpm = useCallback((bpm: number) => {
    const clamped = Math.max(QC_MINIMUM_TEMPO_BPM, Math.min(QC_MAXIMUM_TEMPO_BPM, Math.round(bpm)));
    const expectedMode = settings?.mode ?? "GLOBAL";
    const expectedGlobalBpm = settings?.globalBpm ?? clamped;
    return commit(`Global Tempo ${clamped} BPM`, () => gateway.setGlobalTempo(clamped, expectedMode, expectedGlobalBpm));
  }, [commit, gateway, settings]);

  return {
    open, returnFocusTarget, openTempo, close, loading, pending,
    settings, tempoClock, refresh,
    setLedEnabled, setRunning, setVolumeDb, setPan,
    setTimeSignature, setSubdivision, setSound, setRouting,
    setMode, setGlobalBpm
  };
}

export type TempoWorkflow = ReturnType<typeof useTempoWorkflow>;
