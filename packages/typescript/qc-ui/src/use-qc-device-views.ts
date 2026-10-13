import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { qcDeviceInformation } from "./device-information.ts";
import type { GatewayTransport, PresetList, PresetSnapshot } from "@qc-remote/client";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane.tsx";
import { closeQcLooperScreen } from "./qc-device-screen.ts";
import { closeOnUnit, DEFAULT_SCREEN_POLL, openOnUnit, unitScreen, untouchableUnitScreen, type QcScreenPoll } from "./qc-unit-screens.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";

export type QcDeviceViewName = "tuner" | "gig" | "io" | "info";
export type QcDeviceGesture = "tap" | "long-tap" | "swipe";

export interface QcDeviceViewsOptions {
  sessionKey?: unknown;
  captureSessionCurrent?(): () => boolean;
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
    looper: { run(command: string, value?: number | null): Promise<void | boolean> };
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
  const [deviceInformation, setDeviceInformation] = useState<{ key: unknown; value: CorOsDeviceInformation }>();
  const lifetime = useDeviceWorkflowSession({ ...options, connected: options.session.sessionReady });
  const latest = useRef({ ...lifetime.options, lifetime });
  latest.current = { ...lifetime.options, lifetime };

  const close = useCallback(async () => {
    if (!lifetime.current()) return;
    const { destination, session, gateway, screenPoll, workflows, notice, fail, lifetime: owner } = latest.current;
    if (!owner.current()) return;
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
          const closed = await closeOnUnit(gateway, screenPoll ?? DEFAULT_SCREEN_POLL, "io");
          if (!closed) { notice("The Quad Cortex did not close I/O Settings."); return; }
        } catch (error) {
          fail(error);
          return;
        }
      }
      owner.apply(() => destination.setView(null));
      return;
    }
    if (view === "looper-editor") { await closeQcLooperScreen({ looper: workflows.looper, setView: (next) => owner.apply(() => destination.setView(next)), captureSessionCurrent: () => owner.current }); return; }
    const owned = isGigView(view) ? "gig" : isTunerView(view) ? "tuner" : undefined;
    if (!owned) { destination.setView(null); return; }
    // With nothing to tell (the unit is gone) the app's own view must still be able to close.
    if (await workflows.performance.showDeviceView(owned, false) || (!session.demo && !session.hardwareReady)) owner.apply(() => destination.setView(null));
  }, [lifetime.key]);

  const open = useCallback(async (view: QcDeviceViewName) => {
    if (!lifetime.current()) return;
    const { destination, snapshot, session, gateway, screenPoll, workflows, notice, fail, lifetime: owner } = latest.current;
    if (!owner.current()) return;
    if (view === "tuner" || view === "gig") {
      destination.latch.expectUnit(view);
      if (!await workflows.performance.showDeviceView(view, true)) { owner.apply(() => destination.latch.cancelUnit(view)); return; }
      if (!owner.current()) return;
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
        // Armed just before the drag is sent, so a refusal ("busy") leaves nothing behind to swallow the unit's own I/O Settings.
        const shown = await openOnUnit(gateway, screenPoll ?? DEFAULT_SCREEN_POLL, "io", () => owner.apply(() => destination.latch.expectUnit("io")));
        if (shown !== "shown") {
          notice(shown === "busy"
            ? "Close the screen open on the Quad Cortex before opening I/O Settings."
            : "The Quad Cortex did not show I/O Settings.");
          return;
        }
        if (!owner.current()) return;
        await workflows.ioEq.refresh();
        owner.apply(() => destination.setView("io-overview"));
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
      const identity=await gateway.identity();
      owner.apply(() => setDeviceInformation({ key: owner.key, value: qcDeviceInformation(identity, snapshot.deviceName) }));
    } catch (error) {
      fail(error);
    }
  }, [lifetime.key]);

  // Keep the unit's identity at hand once a session answers, so every window that shows it (the app's own
  // Device Information screen and the Windows Device Info window) has the real values without being asked.
  const sessionReady = options.session.sessionReady;
  useEffect(() => {
    setDeviceInformation(undefined);
    if (!sessionReady) return;
    let current = true;
    const { gateway, snapshot } = lifetime.options;
    gateway.identity()
      .then((identity) => { if (current) lifetime.apply(() => setDeviceInformation({ key: lifetime.key, value: qcDeviceInformation(identity, snapshot.deviceName) })); })
      .catch(() => undefined);
    return () => { current = false; };
  }, [sessionReady, lifetime.key]);

  const toggle = useCallback(async (view: "tuner" | "gig" | "io") => {
    if (!lifetime.current()) return;
    const current = latest.current.destination.view;
    if (view === "gig" ? isGigView(current) : view === "tuner" ? isTunerView(current) : isIoView(current)) await close();
    else await open(view);
  }, [close, open, lifetime.key]);

  const forwardGesture = useCallback(async (x: number, y: number, toX: number, toY: number, kind: QcDeviceGesture) => {
    if (!lifetime.current()) return;
    const { session, gateway, notice, fail, lifetime: owner } = latest.current;
    if (!owner.current()) return;
    if (session.demo) return;
    if (!session.hardwareReady) {
      notice("The Quad Cortex disconnected before the screen gesture could be sent.");
      return;
    }
    try {
      // A screen that can start a download or an install is shown, never touched, by the app.
      const refused = untouchableUnitScreen((await gateway.graphicsTree()).tree);
      if (refused) {
        notice(refused === "Settings"
          ? "The Quad Cortex is showing Settings. Its panes, including Updates, Power Functions and Factory Reset, are display only in the app; use the unit."
          : "The Quad Cortex is showing an update screen. Update screens are display only in the app; use the unit.");
        return;
      }
      if (kind === "long-tap") await gateway.longTapScreen(x, y);
      else if (kind === "tap") await gateway.tapScreen(x, y);
      else await gateway.swipeScreen(x, y, toX, toY);
    } catch (error) {
      fail(error);
    }
  }, [lifetime.key]);

  const currentInformation = deviceInformation?.key === lifetime.key ? deviceInformation.value : undefined;
  return useMemo(() => ({ deviceInformation: currentInformation, open, close, toggle, forwardGesture }), [currentInformation, open, close, toggle, forwardGesture]);
}
