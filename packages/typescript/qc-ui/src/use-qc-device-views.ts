import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { qcDeviceInformation } from "./device-information.ts";
import type { GatewayTransport, PresetList, PresetSnapshot } from "@qc-remote/client";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane.tsx";
import { closeQcLooperScreen } from "./qc-device-screen.ts";
import { closeOnUnit, DEFAULT_SCREEN_POLL, openOnUnit, QC_UPDATE_SCREEN, unitScreen, type QcScreenPoll } from "./qc-unit-screens.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";

export type QcDeviceViewName = "tuner" | "gig" | "io" | "info";
export type QcDeviceGesture = "tap" | "long-tap" | "swipe";

export interface QcDeviceViewsOptions {
  destination: Pick<QcScreenDestination, "view" | "setView" | "latch">;
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
  screenPoll?: QcScreenPoll;
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
  /** Open the screen, or close it if it is already showing. Tuner, Gig View and I/O. */
  toggle(view: "tuner" | "gig" | "io"): Promise<void>;
  /** Forward a tap, long tap or swipe made on a device-owned screen to the unit. */
  forwardGesture(x: number, y: number, toX: number, toY: number, kind: QcDeviceGesture): Promise<void>;
}

const isGigView = (view: string | null | undefined) => Boolean(view?.startsWith("gig"));
const isTunerView = (view: string | null | undefined) => Boolean(view?.startsWith("tuner"));
/** The I/O family: the I/O screens and Global EQ, which the unit shows inside its I/O dialog. */
const isIoView = (view: string | null | undefined) => Boolean(view?.startsWith("io-") || view === "global-eq");

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
          const closed = await closeOnUnit(latest.current.gateway, latest.current.screenPoll ?? DEFAULT_SCREEN_POLL, "io");
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
      destination.latch.expectUnit(view);
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
        destination.latch.expectUnit("io");
        const shown = await openOnUnit(gateway, latest.current.screenPoll ?? DEFAULT_SCREEN_POLL, "io");
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
      setDeviceInformation(qcDeviceInformation(await gateway.identity(), snapshot.deviceName));
    } catch (error) {
      fail(error);
    }
  }, []);

  // Keep the unit's identity at hand once a session answers, so every window that shows it (the app's own
  // Device Information screen and the Windows Device Info window) has the real values without being asked.
  const sessionReady = options.session.sessionReady;
  useEffect(() => {
    if (!sessionReady) return;
    let current = true;
    latest.current.gateway.identity()
      .then((identity) => { if (current) setDeviceInformation(qcDeviceInformation(identity, latest.current.snapshot.deviceName)); })
      .catch(() => undefined);
    return () => { current = false; };
  }, [sessionReady]);

  const toggle = useCallback(async (view: "tuner" | "gig" | "io") => {
    const current = latest.current.destination.view;
    if (view === "gig" ? isGigView(current) : view === "tuner" ? isTunerView(current) : isIoView(current)) await close();
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
      // A screen that can start a download or an install is shown, never touched, by the app.
      if (QC_UPDATE_SCREEN.test((await gateway.graphicsTree()).tree)) {
        notice("The Quad Cortex is showing an update screen. Update screens are display only in the app; use the unit.");
        return;
      }
      if (kind === "long-tap") await gateway.longTapScreen(x, y);
      else if (kind === "tap") await gateway.tapScreen(x, y);
      else await gateway.swipeScreen(x, y, toX, toY);
    } catch (error) {
      fail(error);
    }
  }, []);

  return useMemo(() => ({ deviceInformation, open, close, toggle, forwardGesture }), [deviceInformation, open, close, toggle, forwardGesture]);
}
