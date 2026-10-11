import { useCallback, useMemo, useRef, useState } from "react";
import type { GatewayTransport, PresetList, PresetSnapshot } from "@qc-remote/client";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane.tsx";
import { QC_DEVICE_SCREEN_GESTURES } from "./device-screen-navigation.ts";
import { closeQcLooperScreen } from "./qc-device-screen.ts";
import { classifyQcGraphicsTree, type QcObservedScreenKind } from "./use-adaptive-screen-polling.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";

export type QcDeviceViewName = "tuner" | "gig" | "io" | "info";
export type QcDeviceGesture = "tap" | "long-tap" | "swipe";

export interface QcDeviceViewsOptions {
  destination: Pick<QcScreenDestination, "view" | "setView">;
  snapshot: Pick<PresetSnapshot, "setlistKey" | "deviceName">;
  session: {
    /** A demo or virtual device: nothing is sent to hardware. */
    demo: boolean;
    /** The physical unit answers (connected, not suspended, not in a bulk transfer). */
    hardwareReady: boolean;
    /** Either the physical unit or a demo device answers. */
    sessionReady: boolean;
  };
  gateway: Pick<GatewayTransport, "identity" | "tapScreen" | "longTapScreen" | "swipeScreen" | "graphicsTree">;
  /** How often, and for how long, to read the unit's screen while waiting for it to change. */
  screenPoll?: { intervalMs: number; timeoutMs: number };
  workflows: {
    performance: { showDeviceView(view: "tuner" | "gig", visible?: boolean): Promise<boolean> };
    tuner: { openTuner(): unknown };
    looper: { run(command: string, value?: number | null): Promise<void> };
    ioEq: { refresh(): Promise<unknown> };
    preset: { presetList?: Pick<PresetList, "setlistKey">; loadDirectory(...args: [boolean, string, boolean]): unknown };
  };
  notice(message: string): void;
  fail(error: unknown): void;
}

export interface QcDeviceViews {
  /** The screen-only identity the Device Information screen shows, once it has been read. */
  deviceInformation?: CorOsDeviceInformation;
  /** Open a device screen on the unit and in the app. */
  open(view: QcDeviceViewName): Promise<void>;
  /** Close whichever device screen is showing, telling the unit when it owns that screen. */
  close(): Promise<void>;
  /** Open the screen, or close it if it is already showing. Gig View and I/O. */
  toggle(view: "gig" | "io"): Promise<void>;
  /** Forward a tap, long tap or swipe made on a device-owned screen to the unit. */
  forwardGesture(x: number, y: number, toX: number, toY: number, kind: QcDeviceGesture): Promise<void>;
}

const isGigView = (view: string | null | undefined) => Boolean(view?.startsWith("gig"));
const isTunerView = (view: string | null | undefined) => Boolean(view?.startsWith("tuner"));
/** The I/O family: the I/O screens and Global EQ, which the unit shows inside its I/O dialog. */
const isIoView = (view: string | null | undefined) => Boolean(view?.startsWith("io-") || view === "global-eq");
const DEFAULT_SCREEN_POLL = { intervalMs: 250, timeoutMs: 2_500 };

/**
 * The one set of open, close and toggle workflows for the device screens
 * (Tuner, Gig View, I/O, Device Information, Looper X) and for forwarding
 * gestures made on them. Both hosts go through the same command path
 * (performance.showDeviceView), the same preconditions and the same notices;
 * the demo device is projected locally by that command, never sent to hardware.
 */
export function useQcDeviceViews(options: QcDeviceViewsOptions): QcDeviceViews {
  const [deviceInformation, setDeviceInformation] = useState<CorOsDeviceInformation>();
  const latest = useRef(options);
  latest.current = options;

  const close = useCallback(async () => {
    const { destination, session, workflows, notice, fail } = latest.current;
    const view = destination.view;
    if (isIoView(view)) {
      if (session.demo) {
        destination.setView(null);
        notice("Demo: I/O Settings closed.");
        return;
      }
      if (session.hardwareReady) {
        try {
          // The unit has no command for this screen: it closes on a tap of its check button, and only if it is showing it.
          const closed = await closeIoOnUnit(latest.current);
          if (!closed) { notice("The Quad Cortex did not close I/O Settings."); return; }
        } catch (error) {
          fail(error);
          return;
        }
      }
      destination.setView(null);
      return;
    }
    if (view === "looper-editor") { await closeQcLooperScreen({ looper: workflows.looper, setView: destination.setView }); return; }
    const owned = isGigView(view) ? "gig" : isTunerView(view) ? "tuner" : undefined;
    if (!owned) { destination.setView(null); return; }
    // With nothing to tell (the unit is gone) the app's own view must still be able to close.
    if (await workflows.performance.showDeviceView(owned, false) || (!session.demo && !session.hardwareReady)) destination.setView(null);
  }, []);

  const open = useCallback(async (view: QcDeviceViewName) => {
    const { destination, snapshot, session, gateway, workflows, notice, fail } = latest.current;
    if (view === "tuner" || view === "gig") {
      if (!await workflows.performance.showDeviceView(view, true)) return;
      if (view === "tuner") workflows.tuner.openTuner();
      destination.setView(view);
      // The Gig list is the current setlist; load it when it is missing or belongs to another setlist.
      if (view === "gig" && (!workflows.preset.presetList || workflows.preset.presetList.setlistKey !== snapshot.setlistKey)) {
        void workflows.preset.loadDirectory(false, snapshot.setlistKey, true);
      }
      return;
    }
    if (view === "io") {
      if (session.demo) {
        destination.setView("io-overview");
        notice("Demo: I/O Settings opened.");
        return;
      }
      if (!session.hardwareReady) {
        notice("Connect the Quad Cortex before opening I/O Settings.");
        return;
      }
      try {
        // The unit has no command for this screen: it opens on a long downward drag, but only from its Grid.
        const shown = await openIoOnUnit(latest.current);
        if (shown !== "shown") {
          notice(shown === "busy"
            ? "Close the screen open on the Quad Cortex before opening I/O Settings."
            : "The Quad Cortex did not show I/O Settings.");
          return;
        }
        await workflows.ioEq.refresh();
        destination.setView("io-overview");
        notice("I/O Settings opened.");
      } catch (error) {
        fail(error);
      }
      return;
    }
    if (!session.sessionReady) {
      notice("Connect a Quad Cortex or start the Demo device before opening Device Information.");
      return;
    }
    destination.setView("settings-info");
    try {
      const identity = await gateway.identity();
      setDeviceInformation({
        serial: identity.serial,
        deviceName: identity.customName || snapshot.deviceName,
        corOsVersion: identity.appFwVersion
      });
    } catch (error) {
      fail(error);
    }
  }, []);

  const toggle = useCallback(async (view: "gig" | "io") => {
    const current = latest.current.destination.view;
    if (view === "gig" ? isGigView(current) : isIoView(current)) await close();
    else await open(view);
  }, [close, open]);

  const forwardGesture = useCallback(async (x: number, y: number, toX: number, toY: number, kind: QcDeviceGesture) => {
    const { session, gateway, notice, fail } = latest.current;
    if (session.demo) return;
    if (!session.hardwareReady) {
      notice("The Quad Cortex disconnected before the screen gesture could be sent.");
      return;
    }
    try {
      if (kind === "long-tap") await gateway.longTapScreen(x, y);
      else if (kind === "tap") await gateway.tapScreen(x, y);
      else await gateway.swipeScreen(x, y, toX, toY);
    } catch (error) {
      fail(error);
    }
  }, []);

  return useMemo(() => ({ deviceInformation, open, close, toggle, forwardGesture }), [deviceInformation, open, close, toggle, forwardGesture]);
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** What the unit is showing, read from its graphics tree. */
async function unitScreen(gateway: Pick<GatewayTransport, "graphicsTree">): Promise<QcObservedScreenKind> {
  return classifyQcGraphicsTree((await gateway.graphicsTree()).tree);
}

/** Wait (bounded) until the unit shows a screen satisfying `wanted`. */
async function untilUnitShows(options: QcDeviceViewsOptions, wanted: (kind: QcObservedScreenKind) => boolean): Promise<boolean> {
  const { intervalMs, timeoutMs } = options.screenPoll ?? DEFAULT_SCREEN_POLL;
  for (let waited = 0; waited <= timeoutMs; waited += intervalMs) {
    if (wanted(await unitScreen(options.gateway))) return true;
    await sleep(intervalMs);
  }
  return false;
}

/**
 * Open I/O Settings on the unit with its one drag gesture. The drag is only
 * sent when the unit shows its Grid: on any other screen the same drag could
 * scroll a list or turn a knob. "busy" means another screen is open.
 */
async function openIoOnUnit(options: QcDeviceViewsOptions): Promise<"shown" | "busy" | "not-shown"> {
  const { gateway } = options;
  const current = await unitScreen(gateway);
  if (current === "io") return "shown";
  if (current !== "grid") return "busy";
  const { x, y, toX, toY } = QC_DEVICE_SCREEN_GESTURES.openIo;
  await gateway.swipeScreen(x, y, toX, toY);
  return await untilUnitShows(options, (kind) => kind === "io") ? "shown" : "not-shown";
}

/**
 * Close I/O Settings on the unit with a tap on its check button, only while it
 * shows I/O (a tap elsewhere could press something). Global EQ is drawn inside
 * the same dialog and its check button may only step back, so a still-open
 * dialog gets one more tap before the close is reported as failed.
 */
async function closeIoOnUnit(options: QcDeviceViewsOptions): Promise<boolean> {
  const { gateway } = options;
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const current = await unitScreen(gateway);
    if (current !== "io") return true;
    await gateway.tapScreen(QC_DEVICE_SCREEN_GESTURES.done.x, QC_DEVICE_SCREEN_GESTURES.done.y);
    if (await untilUnitShows(options, (kind) => kind !== "io")) return true;
  }
  return false;
}
