import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { qcDeviceInformation } from "./device-information.ts";
import type { DeviceIdentity, GatewayTransport, PresetList, PresetSnapshot } from "@qc-remote/client";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane.tsx";
import { closeQcLooperScreen } from "./qc-device-screen.ts";
import { closeOnUnit, DEFAULT_SCREEN_POLL, openOnUnit, untouchableUnitScreen, unitScreen, type QcScreenPoll } from "./qc-unit-screens.ts";
import type { QcScreenDestination } from "./use-qc-screen-destination.ts";
import { captureGatewaySession } from "./gateway-session.ts";
import { useDeviceWorkflowSession } from "./use-device-workflow-session.ts";
import type { ObservedCorOsVersion } from "./cor-os-version.ts";
import type { PresetLifetime } from "@qc-remote/core";

export type QcDeviceViewName = "tuner" | "gig" | "io" | "info";
export type QcDeviceGesture = "tap" | "long-tap" | "swipe";

export interface QcDeviceViewsOptions {
  lifetime?: PresetLifetime;
  sessionKey?: unknown;
  captureSessionCurrent?(): () => boolean;
  destination: Pick<QcScreenDestination, "view" | "setView" | "latch" | "captureCurrent">;
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
    looper: { run(command: string, value?: number | null): Promise<boolean>; captureContinuation(): () => boolean };
    ioEq: { refresh(): Promise<unknown> };
    deviceSettings?: { screenCorOsVersion?: ObservedCorOsVersion };
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
  const hostSession = useDeviceWorkflowSession({ ...options, connected: options.session.sessionReady });
  options = hostSession.options;
  const [deviceInformation, setDeviceInformation] = useState<{ key: unknown; value: CorOsDeviceInformation; identity: DeviceIdentity; readAt: number }>();
  const [identitySessionRevision, setIdentitySessionRevision] = useState(0);
  const identityReadGeneration = useRef(0);
  const latest = useRef(options);
  latest.current = options;
  const operation = useRef(0);
  const gestureOperation = useRef(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; operation.current += 1; gestureOperation.current += 1; }; }, []);
  const beginFlow = () => {
    const original = latest.current;
    const generation = ++operation.current;
    const checkpoint = original.lifetime?.checkpoint();
    let destinationCurrent = original.destination.captureCurrent();
    const initialView = original.destination.view;
    let expectedView = initialView;
    let committedExpected = true;
    const viewCurrent = () => {
      const shown = latest.current.destination.view;
      if (shown === expectedView) committedExpected = true;
      // React may not have committed our own setView yet. Once it has, never
      // accept a later return to the old view as belonging to this operation.
      return shown === expectedView || (!committedExpected && shown === initialView);
    };
    const captured = captureGatewaySession(original.gateway as GatewayTransport, () => mounted.current
      && destinationCurrent()
      && operation.current === generation && latest.current.gateway === original.gateway
      && latest.current.session.demo === original.session.demo
      && (!checkpoint || original.lifetime!.currentSession(checkpoint))
      && viewCurrent());
    return { ...captured, setView(view: typeof expectedView) {
      captured.assertCurrent();
      expectedView = view;
      committedExpected = false;
      original.destination.setView(view);
      // Adopt only our own admitted transition. An unrelated ABA never
      // refreshes this lease even if the visible view comes back equal.
      destinationCurrent = original.destination.captureCurrent();
    } };
  };

  const close = useCallback(async () => {
    if (!hostSession.current()) return;
    const captured = beginFlow();
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
          const closed = await closeOnUnit(captured.gateway, latest.current.screenPoll ?? DEFAULT_SCREEN_POLL, "io");
          if (!captured.current()) return;
          if (!closed) { notice("The Quad Cortex did not close I/O Settings."); return; }
        } catch (error) {
          if (captured.current()) fail(error);
          return;
        }
      }
      destination.setView(null);
      return;
    }
    if (view === "looper-editor") { await closeQcLooperScreen({ looper: workflows.looper, setView: (view) => { if (captured.current()) destination.setView(view); } }); return; }
    const owned = isGigView(view) ? "gig" : isTunerView(view) ? "tuner" : undefined;
    if (!owned) { destination.setView(null); return; }
    // With nothing to tell (the unit is gone) the app's own view must still be able to close.
    const closed = await workflows.performance.showDeviceView(owned, false);
    if (captured.current() && (closed || (!session.demo && !session.hardwareReady))) destination.setView(null);
  }, [hostSession.key]);

  const open = useCallback(async (view: QcDeviceViewName) => {
    if (!hostSession.current()) return;
    const captured = beginFlow();
    const { destination, snapshot, session, workflows, notice, fail } = latest.current;
    const gateway = captured.gateway;
    if (view === "tuner" || view === "gig") {
      destination.latch.expectUnit(view);
      const shown = await workflows.performance.showDeviceView(view, true);
      if (!captured.current()) return;
      if (!shown) { destination.latch.cancelUnit(view); return; }
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
        const shown = await openOnUnit(gateway, latest.current.screenPoll ?? DEFAULT_SCREEN_POLL, "io", () => { captured.assertCurrent(); destination.latch.expectUnit("io"); });
        if (!captured.current()) return;
        if (shown !== "shown") {
          notice(shown === "busy"
            ? "Close the screen open on the Quad Cortex before opening I/O Settings."
            : "The Quad Cortex did not show I/O Settings.");
          return;
        }
        await workflows.ioEq.refresh();
        if (!captured.current()) return;
        destination.setView("io-overview");
        notice("I/O Settings opened.");
      } catch (error) {
        if (captured.current()) fail(error);
      }
      return;
    }
    if (!session.sessionReady) {
      notice("Connect a Quad Cortex or start the Demo device before opening Device Information.");
      return;
    }
    captured.setView("settings-info");
    try {
      const identity = await gateway.identity();
      if (!captured.current()) return;
      setDeviceInformation({ key: hostSession.key, value: qcDeviceInformation(identity, snapshot.deviceName), identity, readAt: Date.now() });
    } catch (error) {
      if (captured.current()) fail(error);
    }
  }, [hostSession.key]);

  // Keep the unit's identity at hand once a session answers, so every window that shows it (the app's own
  // Device Information screen and the Windows Device Info window) has the real values without being asked.
  const sessionReady = options.session.sessionReady;
  useEffect(() => {
    const lifetime = options.lifetime;
    if (!lifetime) return;
    let origin = lifetime.checkpoint();
    return lifetime.subscribe(() => {
      if (lifetime.currentSession(origin)) return;
      origin = lifetime.checkpoint();
      identityReadGeneration.current++;
      if (!mounted.current) return;
      setDeviceInformation(undefined);
      setIdentitySessionRevision(version => version + 1);
    });
  }, [options.lifetime]);
  useEffect(() => {
    const readGeneration = ++identityReadGeneration.current;
    setDeviceInformation(undefined);
    if (!sessionReady) return;
    let current = true;
    const source = options.gateway;
    const name = options.snapshot.deviceName;
    const checkpoint = options.lifetime?.checkpoint();
    const captured = captureGatewaySession(source as GatewayTransport, () => current && mounted.current
      && readGeneration === identityReadGeneration.current && latest.current.gateway === source
      && latest.current.lifetime === options.lifetime && latest.current.session.sessionReady
      && (!checkpoint || options.lifetime!.currentSession(checkpoint)));
    captured.gateway.identity()
      .then((identity) => {
        if (!captured.current()) return;
        const observed = qcDeviceInformation(identity, name);
        setDeviceInformation({ key: hostSession.key, value: observed, identity, readAt: Date.now() });
      })
      .catch(() => undefined);
    return () => { current = false; };
  }, [sessionReady, options.gateway, options.snapshot.deviceName, options.lifetime, identitySessionRevision, hostSession.key]);

  const toggle = useCallback(async (view: "tuner" | "gig" | "io") => {
    const current = latest.current.destination.view;
    if (view === "gig" ? isGigView(current) : view === "tuner" ? isTunerView(current) : isIoView(current)) await close();
    else await open(view);
  }, [close, open]);

  const forwardGesture = useCallback(async (x: number, y: number, toX: number, toY: number, kind: QcDeviceGesture) => {
    if (!hostSession.current()) return;
    const { session, gateway: source, notice, fail } = latest.current;
    const lifetime = latest.current.lifetime;
    const checkpoint = lifetime?.checkpoint();
    // A later screen gesture owns navigation intent even if the outer I/O
    // view is unchanged. Its predecessor must not dispatch after its safety
    // read finishes. Don't fence by inner-pane generation: the admitted
    // gesture's own local click legitimately changes that pane before await.
    const generation = ++gestureOperation.current;
    const destinationCurrent = latest.current.destination.captureCurrent();
    const captured = captureGatewaySession(source as GatewayTransport, () => mounted.current && gestureOperation.current === generation && destinationCurrent() && latest.current.gateway === source && !latest.current.session.demo && latest.current.session.hardwareReady && (!checkpoint || lifetime!.currentSession(checkpoint)));
    const gateway = captured.gateway;
    if (session.demo) return;
    if (!session.hardwareReady) {
      notice("The Quad Cortex disconnected before the screen gesture could be sent.");
      return;
    }
    try {
      // A screen that can start a download or an install is shown, never touched, by the app.
      const screen = await gateway.graphicsTree();
      captured.assertCurrent();
      const refused = untouchableUnitScreen(screen.tree);
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
      if (captured.current()) fail(error);
    }
  }, [hostSession.key]);

  const screenCorOsVersion = options.workflows.deviceSettings?.screenCorOsVersion;
  const currentRecord = hostSession.current() && deviceInformation?.key === hostSession.key ? deviceInformation : undefined;
  const currentInformation = useMemo(() => !currentRecord ? undefined
    : !screenCorOsVersion ? currentRecord.value
    : { ...currentRecord.value, corOsVersion: qcDeviceInformation(currentRecord.identity, undefined, { identityReadAt: currentRecord.readAt, screen: screenCorOsVersion }).corOsVersion },
  [currentRecord, screenCorOsVersion]);
  return useMemo(() => ({ deviceInformation: currentInformation, open, close, toggle, forwardGesture }), [currentInformation, open, close, toggle, forwardGesture]);
}
