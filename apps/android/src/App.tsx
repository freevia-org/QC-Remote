import { RemoteAccountDeletionLink } from "@qc-remote/ui";
import { openRemoteAccountDeletionPage } from "./native-services";
import { Capacitor } from "@capacitor/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { QC_SCENE_COUNT } from "@qc-remote/client";
import { demoSnapshot, footswitchLeds, sceneLetter, type ControlAccessMode, type PublicRelayUsage } from "@qc-remote/core";
import { formFactors, skins } from "@qc-remote/form-factors";
import { QC_BRAND, QC_COLORS, QC_LEGAL, QC_LEGAL_DOCUMENTS, QC_VISUAL_ASSETS } from "@qc-remote/theme";
import { ControlAccessSelect, consumeQcNativeStateFrame, corosFixtureConfiguration, createGridRepairCoordinator, refreshGridSnapshotOnce, createQcDeviceDisplay, QcDeviceDisplay, QcDeviceDisplayOverlay, createDemoDeviceSession, demoLibraryCounts, IDLE_PLACEHOLDER_SNAPSHOT, connectingPlaceholderSnapshot, isConnectingPlaceholder, parseDemoLibraryText, usePresetMidiOutWorkflow, qcConnectionIndicatorPhase, qcLaneControlPanelBindings, qcParameterEditorBindings, qcRelayPresentation, QcHardwareSwitch, QcMasterVolumeKnob, QcUiIcon, readControlAccessMode, refreshGridSnapshot, serializeDemoLibrary, useBlockEditorSession, useContinuousControlWorkflow, useDeviceSettingsWorkflow, useExpressionAssignment, useIoEqWorkflows, useLooperXWorkflow, useModesConfigurationWorkflow, useNativeStateRecovery, usePublicRelayWorkflow, useQcConnectionWorkflow, useQcController, useQcCpuTelemetry, useQcDeviceUndo, useQcDeviceViews, useQcScreenFollow, useQcFollowSetting, consumeQcDisplayStates, useQcScreenDestination, useQcLiveState, useQcSurfaceActions, useQcWorkflows, useTempoWorkflow, useTunerWorkflow, useWriteHealth, virtualDeviceStoreOf, writeControlAccessMode, writeQcScreenFollowSetting } from "@qc-remote/ui";
import { CorOsProductionScreenAudit } from "@qc-remote/ui/dev";
import { androidGatewayTransport, androidUsbRuntimeDiagnostics, createAndroidQcTransport, publicRelay, QcUsbNative, ScreenWakeNative, subscribeRelayState, virtualDeviceStorage } from "./native-services";
import { useAndroidPrompts } from "./use-android-prompts";
import { OwnedAttemptLatch } from "./owned-attempt-latch.ts";
import { startRelayReportingFailure } from "./relay-start.ts";
import appPackage from "../package.json";
import { qcStreamRestartClearsBulkOperation } from "@qc-remote/ui";

const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const { enabled: corpusFixtureEnabled, screenView: fixtureScreenView, initialSnapshot: fixtureInitialSnapshot } =
  corosFixtureConfiguration(import.meta.env.DEV ? window.location.search : "", demoSnapshot);
const productionAuditScreen = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("productionScreen") : null;
const sceneFootswitches = Array.from({ length: QC_SCENE_COUNT }, (_, index) => ({ index, label: sceneLetter(index) }));
const androidKeepScreenAwakeStorageKey = "qc-control.android-keep-screen-awake-v1";
const screenDimAfterMs = 90_000;
const legacyControlAccessModeKey = "qc-control.device-access-mode-v1";
const storedControlAccessMode = (): ControlAccessMode => readControlAccessMode(window.localStorage, [legacyControlAccessModeKey]);
function AppMark({ onClick }: { onClick: () => void }) {
  return <button type="button" className="app-mark" onClick={onClick} aria-label={`About ${QC_BRAND.appName}`}><img src={QC_VISUAL_ASSETS.appIcon.url} alt="" /></button>;
}

export function App() {
  const native = Capacitor.isNativePlatform();
  const qcController = useQcController(demoSnapshot);
  const {
    snapshot, snapshotRef, setSnapshot, updateSnapshot,
    resetCommands, reconcileFrame
  } = qcController;
  useEffect(() => { if (corpusFixtureEnabled) setSnapshot(fixtureInitialSnapshot); }, [setSnapshot]);
  const demoDeviceSession = useMemo(() => createDemoDeviceSession(androidGatewayTransport), []);
  const activeGateway = demoDeviceSession.gateway;
  const activeSourceRef = useRef<"device" | "virtual">("device");
  activeSourceRef.current = demoDeviceSession.source;
  const qcTransport = useMemo(() => createAndroidQcTransport(activeGateway, () => snapshotRef.current), [activeGateway, snapshotRef]);
  const [selectedBlockId, setSelectedBlockId] = useState("");
  const gridRefreshRequest = useRef(0);
  const editor = useBlockEditorSession();
  const { details: blockDetails } = editor;
  const [parameterMenuInitiallyOpen, setParameterMenuInitiallyOpen] = useState(false);
  const [devicePending, setDevicePending] = useState(false);
  const deviceConnection = useQcConnectionWorkflow({
    phase: native ? "discovering" : "disconnected",
    detail: native ? "Looking for the Quad Cortex…" : "Android USB is unavailable in browser preview.",
    demo: true,
    source: "device"
  });
  const { connection, setConnection, transition: transitionConnection, attempts: connectionAttempts, busy: usbBusy } = deviceConnection;
  const [hardwareConnected, setHardwareConnected] = useState(false);
  const [deviceSuspended, setDeviceSuspended] = useState(false);
  const [deviceBulkBlocking, setDeviceBulkBlocking] = useState(false);
  const [catalogRevision, setCatalogRevision] = useState(0);
  const demoActive = connection.source === "virtual" && connection.phase === "ready";
  const sessionReady = demoActive || (hardwareConnected && !deviceSuspended && !deviceBulkBlocking);
  const [demoPending, setDemoPending] = useState(false);
  const demoStartInFlight = useRef(false);
  // Automatic fallback Demo yields to a newly detected QC. A Demo session the
  // user deliberately selected stays active until they select USB.
  const demoCanYieldToDevice = useRef(true);
  // Native availability may be published while connect() is still in flight.
  // Remember that edge and retry once if the owning attempt does not connect.
  const usbRetryRequested = useRef(false);
  const resumeHardwareInFlight = useRef(false);
  const [notice, setNotice] = useState("Connect the Quad Cortex by USB. Use REMOTE to let an AI app control it.");
  const [gridRefreshPending, setGridRefreshPending] = useState(false);
  const [usbWritesQuarantined, setUsbWritesQuarantined] = useState(false);
  const { observe: observeWriteHealth } = useWriteHealth({
    enabled: native && hardwareConnected,
    recoveryInstruction: "the device is reconnected",
    onNotice: setNotice,
    onQuarantineChange: setUsbWritesQuarantined
  });
  const { prompts: appPrompts, overlay: textPromptOverlay } = useAndroidPrompts();
  const [keepScreenAwake, setKeepScreenAwake] = useState(() => window.localStorage.getItem(androidKeepScreenAwakeStorageKey) !== "false");
  const { follow: followQcScreen, setFollow: setFollowQcScreen } = useQcFollowSetting(window.localStorage);
  const [screenDimmed, setScreenDimmed] = useState(false);
  const [controlAccessMode, setControlAccessMode] = useState<ControlAccessMode>(storedControlAccessMode);
  const relayWorkflow = usePublicRelayWorkflow({ relay: publicRelay, enabled: native, autoStart: hardwareConnected, pollIntervalMs: 2_000, subscribe: subscribeRelayState });
  const relayPaired = relayWorkflow.status?.paired ?? false;
  const [relayUsage, setRelayUsage] = useState<PublicRelayUsage>();
  const [browserRelayPairing, setBrowserRelayPairing] = useState<{ expiresAt: number }>();
  const [browserRelayPairingPending, setBrowserRelayPairingPending] = useState(false);
  const remotePresentation = qcRelayPresentation(relayWorkflow.status, Boolean(browserRelayPairing));
  const usbIndicatorPhase = qcConnectionIndicatorPhase(connection, hardwareConnected && !demoActive, null, { actionRequired: usbWritesQuarantined });
  const usbStatusText = usbIndicatorPhase === "ready"
    ? "Quad Cortex ready"
    : usbIndicatorPhase === "syncing"
      ? "Quad Cortex synchronizing"
      : usbIndicatorPhase === "action-required"
        ? "Quad Cortex writes paused"
      : usbIndicatorPhase === "error"
        ? "Quad Cortex connection error"
        : "Quad Cortex disconnected";
  const [workflowPanel, setWorkflowPanel] = useState<"usb" | "remote" | "about" | "display" | "privacy" | "legal" | "notices" | null>(null);
  const [usbPanelDetail, setUsbPanelDetail] = useState("Checking direct USB status…");
  const screenDestination = useQcScreenDestination();
  const mobileScreenView = screenDestination.view;
  const setMobileScreenView = screenDestination.setView;
  const cpuTelemetry = useQcCpuTelemetry();
  const modeChord = useRef<{ active: boolean; held: boolean; timer?: number }>({ active: false, held: false });
  const controlContacts = useRef({ bankDown: false, tempo: false });
  const suppressControlTapUntil = useRef({ bankDown: 0, tempo: 0 });
  const connectAttemptLatch = useRef(new OwnedAttemptLatch());
  const presetSynchronized = useRef(false);
  const usbSessionReady = useRef(false);
  const nativeStateSequence = useRef(0);
  const nativeStateStream = useRef<string | undefined>(undefined);
  const nativeStateRecoveryRevision = useRef(0);
  const gridRepair = useRef(createGridRepairCoordinator());
  const startDemoDeviceRef = useRef<(reason?: string) => Promise<void>>(async () => undefined);
  const resumeHardwareDeviceRef = useRef<(knownConnected?: boolean) => Promise<void>>(async () => undefined);
  useEffect(() => () => {
    if (modeChord.current.timer !== undefined) window.clearTimeout(modeChord.current.timer);
  }, []);
  useNativeStateRecovery({
    enabled: native && hardwareConnected && !demoActive && !corpusFixtureEnabled,
    controller: qcController,
    sequence: nativeStateSequence,
    streamId: nativeStateStream,
    recoveryRevision: nativeStateRecoveryRevision,
    gridRepair,
    readEvents: (after, limit) => androidGatewayTransport.currentStateEvents(after, limit),
    readSnapshot: () => androidGatewayTransport.currentSnapshot(),
    readGridRepair: () => androidGatewayTransport.currentPreset()
  });
  const screenDimTimer = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (!native) return;
    const enabled = keepScreenAwake && hardwareConnected;
    void ScreenWakeNative.setEnabled({ enabled });
    return () => { void ScreenWakeNative.setEnabled({ enabled: false }); };
  }, [keepScreenAwake, native, hardwareConnected]);
  const changeKeepScreenAwake = (enabled: boolean) => {
    setKeepScreenAwake(enabled);
    window.localStorage.setItem(androidKeepScreenAwakeStorageKey, String(enabled));
  };
  const changeFollowQcScreen = (enabled: boolean) => {
    setFollowQcScreen(enabled);
    setNotice(enabled ? "Following supported Quad Cortex screen changes while QC Remote is visible." : "Quad Cortex screen following is off.");
  };
  const resetScreenDimmer = useCallback(() => {
    if (!native || !keepScreenAwake || !hardwareConnected) return;
    if (screenDimTimer.current !== undefined) window.clearTimeout(screenDimTimer.current);
    setScreenDimmed(false);
    screenDimTimer.current = window.setTimeout(() => setScreenDimmed(true), screenDimAfterMs);
  }, [keepScreenAwake, native, hardwareConnected]);
  useEffect(() => {
    if (!native || !keepScreenAwake || !hardwareConnected) {
      if (screenDimTimer.current !== undefined) window.clearTimeout(screenDimTimer.current);
      screenDimTimer.current = undefined;
      setScreenDimmed(false);
      return;
    }
    resetScreenDimmer();
    return () => { if (screenDimTimer.current !== undefined) window.clearTimeout(screenDimTimer.current); };
  }, [keepScreenAwake, native, resetScreenDimmer, hardwareConnected]);
  const workflows = useQcWorkflows({
    controller: qcController,
    transport: qcTransport,
    gateway: activeGateway,
    editor,
    selectedBlockId,
    setSelectedBlockId,
    connected: corpusFixtureEnabled || sessionReady,
    demo: corpusFixtureEnabled || demoActive || !native,
    source: connection.source,
    pending: devicePending,
    setPending: setDevicePending,
    prompts: appPrompts,
    panels: {
      // Routing's advanced dialog, block management, and the generic scenes
      // dialog are all retired the same way Windows retired them (see
      // scene-editor.tsx's and routing-editor.tsx's own module comments) -
      // nothing on Android ever called routingWorkflow.open() or
      // gridWorkflow.openManagement() either, so there was never a reachable
      // "routing"/"block" panel here to begin with. Add Device now opens the
      // unit's own live device browser over the Grid, exactly like Windows;
      // scene copy/swap/label/color now open the device-accurate overlays
      // below instead of the retired generic dialog.
      liveAddDevicePicker: true,
      close: () => setWorkflowPanel(null)
    },
    notice: setNotice,
    fail: (error) => setNotice(error instanceof Error ? error.message : String(error)),
    // Deliberately notice-only, as Windows is. A failed device command says
    // nothing about the USB session: the plugin owns that state and publishes
    // its loss through qcConnection. Calling transitionConnection("error")
    // here reported a session that was still streaming as disconnected, and
    // because nothing re-derives the flag the screen stayed wrong until the
    // person pressed Connect on a device that had never gone away.
    performanceFail: (error) => {
      setNotice(error instanceof Error ? error.message : String(error));
    },
    catalogRevision
  });
  const {
    reconcile: reconcileWorkflowSnapshot,
    history: deviceHistory,
    preset: presetWorkflow,
    routing: routingWorkflow,
    grid: gridWorkflow,
    parameter: parameterWorkflow,
    scene: sceneWorkflow,
    performance: performanceWorkflow
  } = workflows;
  const continuousControls = useContinuousControlWorkflow({
    controller: qcController,
    gateway: activeGateway,
    connected: sessionReady,
    demo: corpusFixtureEnabled || demoActive || !native,
    reconcile: reconcileWorkflowSnapshot,
    recordHistory: deviceHistory.record,
    notice: setNotice,
    fail: (error) => setNotice(error instanceof Error ? error.message : String(error))
  });
  const deviceSettingsWorkflow = useDeviceSettingsWorkflow({
    gateway: activeGateway,
    connected: corpusFixtureEnabled || sessionReady,
    pending: devicePending,
    setPending: setDevicePending,
    notice: setNotice,
    fail: (error) => setNotice(error instanceof Error ? error.message : String(error))
  });
  const tempoWorkflow = useTempoWorkflow({
    gateway: activeGateway,
    connected: corpusFixtureEnabled || sessionReady,
    pending: devicePending,
    setPending: setDevicePending,
    notice: setNotice,
    fail: (error) => setNotice(error instanceof Error ? error.message : String(error))
  });
  const workflowFailed = (error: unknown) => setNotice(error instanceof Error ? error.message : String(error));
  const tunerWorkflow = useTunerWorkflow({ gateway: activeGateway, connected: sessionReady, pending: devicePending, setPending: setDevicePending, notice: setNotice, fail: workflowFailed });
  const looperWorkflow = useLooperXWorkflow({ gateway: activeGateway, connected: sessionReady, pending: devicePending, setPending: setDevicePending, notice: setNotice, fail: workflowFailed });
  const midiOutWorkflow = usePresetMidiOutWorkflow({ gateway: activeGateway, snapshot, demo: demoActive, onSaved: reconcileWorkflowSnapshot });
  const modesWorkflow = useModesConfigurationWorkflow({ gateway: activeGateway, connected: sessionReady, pending: devicePending, setPending: setDevicePending, notice: setNotice, fail: workflowFailed });
  const ioEqWorkflow = useIoEqWorkflows({ gateway: activeGateway, connected: sessionReady, pending: devicePending, setPending: setDevicePending, notice: setNotice, fail: workflowFailed, snapshot });
  const deviceUndo = useQcDeviceUndo({ history: deviceHistory, snapshot, gateway: activeGateway, fail: workflowFailed });
  const deviceViews = useQcDeviceViews({
    destination: screenDestination, snapshot, gateway: activeGateway, notice: setNotice, fail: workflowFailed,
    session: { demo: demoActive, hardwareReady: native && hardwareConnected && !deviceSuspended && !deviceBulkBlocking, sessionReady },
    workflows: { performance: performanceWorkflow, tuner: tunerWorkflow, looper: looperWorkflow, ioEq: ioEqWorkflow, preset: presetWorkflow }
  });
  useQcScreenFollow({
    gateway: activeGateway, follow: followQcScreen, available: native && !demoActive && !corpusFixtureEnabled, ready: hardwareConnected && !deviceSuspended && !deviceBulkBlocking, destination: screenDestination, notice: setNotice,
    workflows: { grid: gridWorkflow, preset: presetWorkflow, deviceSettings: deviceSettingsWorkflow, tempo: tempoWorkflow, tuner: tunerWorkflow, ioEq: ioEqWorkflow, modes: modesWorkflow }
  });
  useEffect(() => {
    if (!sessionReady) return;
    void deviceSettingsWorkflow.refresh();
  }, [deviceSettingsWorkflow.refresh, sessionReady]);
  // Live Expression 1/2 pedal assignment, ported from Windows (App.tsx's own
  // expressionWorkflow) - shared qc-ui code, unmodified. Previously
  // "Assign Expression Pedal" opened the retired GridManagementPanel, which
  // never had an expression-pedal control on it at all; this reaches the
  // real device-accurate assignment screen instead.
  const expressionWorkflow = useExpressionAssignment({
    gateway: activeGateway,
    snapshot,
    connected: sessionReady,
    pending: devicePending,
    setPending: setDevicePending,
    reconcile: reconcileWorkflowSnapshot,
    recordHistory: deviceHistory.record,
    notice: setNotice,
    fail: (error) => setNotice(error instanceof Error ? error.message : String(error))
  });
  const parameterEditorBindings = qcParameterEditorBindings({
    titlePresetMark: corpusFixtureEnabled && editor.details?.name === "Simple Gate",
    snapshot,
    selectedBlockId,
    editor,
    grid: gridWorkflow,
    parameter: parameterWorkflow,
    performance: performanceWorkflow,
    connected: sessionReady,
    pending: devicePending,
    initialMenuOpen: parameterMenuInitiallyOpen,
    notice: setNotice,
    openExpression: () => { if (blockDetails) expressionWorkflow.openAssignment(blockDetails); }
  });
  // Input Gate / Lane Output live editor, ported from Windows the same way.
  const laneControlEditor = qcLaneControlPanelBindings({
    snapshot,
    selectedBlockId,
    editor,
    grid: gridWorkflow,
    performance: performanceWorkflow,
    pending: devicePending,
    notice: setNotice,
    openExpression: (control) => { if (blockDetails) expressionWorkflow.openAssignment(blockDetails, { kind: "lane", control }); }
  });
  const expressionAssignment = blockDetails ? expressionWorkflow.assignmentFor(blockDetails) : undefined;
  const switchLeds = useMemo(() => footswitchLeds(snapshot), [snapshot]);
  const consumeLiveState = useQcLiveState({
    reconcileFrame,
    editor,
    onStates: (states, frame) => {
      consumeQcDisplayStates(states, { setView: setMobileScreenView, cpu: cpuTelemetry, undo: deviceUndo, follow: followQcScreen });
      for (const state of states) {
        if (state.kind === "gridTopology" && state.gridLayoutComplete === false
            && activeSourceRef.current !== "virtual") {
          // A sparse structural echo cannot safely identify a newly added
          // block. Reconcile once through the explicit current-preset path;
          // ordinary cached snapshots remain frame-free and this read is
          // bounded to one request for a complete publication.
          editor.close();
          setSelectedBlockId("");
          const attempt = connectionAttempts.capture();
          void refreshGridSnapshotOnce(gridRepair.current, frame?.streamId ?? nativeStateStream.current, frame?.sequence, {
            sequence: gridRefreshRequest,
            readSnapshot: qcController.readSnapshot,
            read: () => androidGatewayTransport.currentPreset(),
            isCurrent: () => attempt.current() && activeSourceRef.current !== "virtual",
            onPending: () => setGridRefreshPending(true),
            onError: (error) => setNotice(error instanceof Error ? error.message : "The Grid layout is incomplete; current preset could not be refreshed."),
            onFinished: () => setGridRefreshPending(false)
          });
        } else if (state.kind === "suspend" && state.suspended !== undefined) {
          setDeviceSuspended(state.suspended);
          setNotice(state.suspended ? `Quad Cortex temporarily suspended control${state.suspendReason ? ` (${state.suspendReason.replaceAll("_", " ").toLowerCase()})` : ""}.` : "Quad Cortex control resumed.");
        } else if (state.kind === "refresh" && state.refreshRequired) {
          void androidGatewayTransport.currentSnapshot().then(setSnapshot).catch(() => undefined);
        } else if (state.kind === "catalog" && state.catalogRefresh) {
          setCatalogRevision((revision) => revision + 1);
          setNotice("The Quad Cortex model catalog changed. Add Device will reload it when opened.");
        } else if (state.kind === "bulkOperation" && state.bulkOperation) {
          const blocking = Boolean(state.bulkOperation.blocking && !state.bulkOperation.finished);
          setDeviceBulkBlocking(blocking);
          if (state.bulkOperation.message) setNotice(state.bulkOperation.message);
        }
      }
      if (states.some((state) => state.kind === "preset")) {
        setHardwareConnected(true);
        if (activeSourceRef.current === "virtual") {
          if (demoCanYieldToDevice.current) void resumeHardwareDeviceRef.current(true);
          return;
        }
        presetSynchronized.current = true;
        // A preset frame can only come from a live native session, which is
        // stronger evidence than the connect() promise that sets this flag.
        // Requiring the flag first meant a failed connect latched the screen:
        // the catch below clears it, and nothing else ever sets it again, so
        // device state streamed in behind a "needs attention" indicator with
        // no way back.
        usbSessionReady.current = true;
        transitionConnection("connected");
      }
    }
  });

  const attemptUsbConnection = async (announce = false) => {
    if (!native) return;
    const latchOwner = connectAttemptLatch.current.acquire();
    if (latchOwner === undefined) {
      usbRetryRequested.current = true;
      return;
    }
    usbRetryRequested.current = false;
    const fromDemo = activeSourceRef.current === "virtual";
    const attempt = connectionAttempts.begin();
    if (fromDemo) {
      demoDeviceSession.useDevice();
      activeSourceRef.current = "device";
      demoCanYieldToDevice.current = false;
      deviceHistory.clear();
      presetWorkflow.resetCache();
      setSelectedBlockId("");
      // The demo's preset must not sit on screen as if it were active while the hardware is being reached.
      setSnapshot(connectingPlaceholderSnapshot("opening"));
    }
    presetSynchronized.current = false;
    nativeStateSequence.current = 0;
    nativeStateStream.current = undefined;
    gridRepair.current = createGridRepairCoordinator();
    transitionConnection("connecting");
    try {
      // Only a deliberate press may raise a permission dialog. An attach while
      // the app is running already puts Android's own confirmation on screen,
      // and asking as well showed two prompts for one plug. The system's grant
      // comes back as a fresh availability event, which retries this.
      const result = await QcUsbNative.connect({ allowPermissionPrompt: announce });
      if (!attempt.current()) return;
      usbSessionReady.current = true;
      setHardwareConnected(true);
      if (result.synchronized) presetSynchronized.current = true;
      transitionConnection(presetSynchronized.current ? "connected" : "syncing");
      if (!presetSynchronized.current && isConnectingPlaceholder(snapshotRef.current)) setSnapshot(connectingPlaceholderSnapshot("synchronizing"));
      if (announce) setNotice("Quad Cortex connected directly over USB.");
    } catch (error) {
      if (!attempt.current()) return;
      usbSessionReady.current = false;
      setHardwareConnected(false);
      // Waiting on a grant is not a fault, and painting it as one is the same
      // lie as reporting a live session dead. Stay on "absent" until the
      // permission lands, which arrives as another availability event.
      const awaitingPermission = (error as { code?: string } | null)?.code === "USB_PERMISSION_REQUIRED";
      transitionConnection(awaitingPermission ? "absent" : "error");
      const primary = error instanceof Error ? error.message : "Could not connect to the Quad Cortex.";
      if (announce) {
        try {
          const diagnostics = await usbDiagnostics();
          if (attempt.current()) setNotice(`${primary} ${diagnostics}`);
        } catch { if (attempt.current()) setNotice(primary); }
      }
      if (fromDemo && attempt.current()) void startDemoDeviceRef.current(primary);
    } finally {
      const released = connectAttemptLatch.current.release(latchOwner);
      if (released && usbRetryRequested.current && !usbSessionReady.current
          && (activeSourceRef.current !== "virtual" || demoCanYieldToDevice.current)) {
        usbRetryRequested.current = false;
        void attemptUsbConnection(false);
      }
    }
  };

  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    const scanAttempt = connectionAttempts.capture();
    const listenerPromises = [
      QcUsbNative.addListener("qcConnection", ({ state }) => {
        if (cancelled) return;
        if (state === "available") {
          if (activeSourceRef.current !== "virtual" || demoCanYieldToDevice.current) void attemptUsbConnection();
        } else {
          connectionAttempts.invalidate();
          connectAttemptLatch.current.clear();
          usbRetryRequested.current = false;
          usbSessionReady.current = false;
          presetSynchronized.current = false;
          nativeStateSequence.current = 0;
          nativeStateStream.current = undefined;
          gridRepair.current = createGridRepairCoordinator();
          setHardwareConnected(false);
          resetCommands();
          if (activeSourceRef.current !== "virtual") {
            transitionConnection("absent");
            void startDemoDeviceRef.current("The Quad Cortex was disconnected.");
          }
        }
      }),
      QcUsbNative.addListener("qcStateBatch", (frame) => {
        if (cancelled) return;
        setHardwareConnected(true);
        if (activeSourceRef.current === "virtual") {
          if (demoCanYieldToDevice.current) void resumeHardwareDeviceRef.current(true);
          return;
        }
        consumeQcNativeStateFrame(frame, {
          sequence: nativeStateSequence,
          streamId: nativeStateStream,
          recoveryRevision: nativeStateRecoveryRevision,
          consume: consumeLiveState,
          setSnapshot,
          // A restarted native stream numbers its frames from the beginning
          // again, and frames are deltas: read the whole state back so the
          // screen rejoins the device where it actually is.
          onStreamRestart: (reason) => {
            resetCommands();
            if (qcStreamRestartClearsBulkOperation(reason)) {
              setDeviceBulkBlocking(false);
            }
          }
        });
      })
    ];
    // Register every native listener before scanning/handshaking so no initial
    // preset frame can beat the Capacitor bridge subscription.
    void Promise.all(listenerPromises).then(async () => {
      if (cancelled || !scanAttempt.current()) return;
      const { devices, connected, synchronized } = await QcUsbNative.scan();
      if (cancelled || !scanAttempt.current()) return;
      if (connected) {
        usbSessionReady.current = true;
        setHardwareConnected(true);
        // Raise, never lower -- as the connect path at attemptUsbConnection
        // already does. A preset frame can arrive while this scan is still in
        // flight, and it is the more recent observation; assigning the scan's
        // older value discarded it. The session then sat synchronized with the
        // screen parked on "syncing", because the only other route to
        // "connected" is a preset frame and the QC sends one only when the
        // preset changes. Reattaching USB mid-handshake reproduced it every
        // time: connected, streaming, and showing disconnected.
        if (synchronized) presetSynchronized.current = true;
        if (activeSourceRef.current === "virtual") {
          if (demoCanYieldToDevice.current) void resumeHardwareDeviceRef.current(true);
        } else {
          transitionConnection(presetSynchronized.current ? "connected" : "syncing");
        }
        return;
      }
      if (!devices.length) {
        setHardwareConnected(false);
        if (activeSourceRef.current !== "virtual") {
          transitionConnection("absent");
          void startDemoDeviceRef.current("No Quad Cortex was found.");
        }
        return;
      }
      void attemptUsbConnection();
    }).catch((error) => {
      if (!cancelled && scanAttempt.current()) {
        transitionConnection("error");
        void startDemoDeviceRef.current(error instanceof Error ? error.message : "USB discovery failed.");
      }
    });
    return () => {
      cancelled = true;
      connectionAttempts.invalidate();
      connectAttemptLatch.current.clear();
      usbRetryRequested.current = false;
      for (const promise of listenerPromises) void promise.then((listener) => listener.remove());
    };
  }, [native]);

  useEffect(() => {
    if (!relayWorkflow.status) return;
    setControlAccessMode(relayWorkflow.status.accessMode);
    writeControlAccessMode(window.localStorage, relayWorkflow.status.accessMode);
  }, [relayWorkflow.status?.accessMode]);

  const refreshRelayUsage = useCallback(async () => {
    if (!native || !relayWorkflow.status?.paired) { setRelayUsage(undefined); return; }
    try { setRelayUsage(await publicRelay.usage()); } catch { setRelayUsage(undefined); }
  }, [native, relayWorkflow.status?.paired]);

  useEffect(() => {
    if (!native || !browserRelayPairing) return;
    let disposed = false;
    let polling = false;
    const poll = async () => {
      if (polling || disposed) return;
      polling = true;
      try {
        const result = await publicRelay.browserPairing!.poll();
        if (disposed) return;
        if (result.state === "paired") {
          setBrowserRelayPairing(undefined);
          await relayWorkflow.refresh();
          setNotice("Phone paired. The secure remote relay is connecting in the background.");
        }
      } catch (error) {
        if (!disposed) {
          setBrowserRelayPairing(undefined);
          setNotice(error instanceof Error ? error.message : "Browser pairing could not be completed.");
        }
      } finally { polling = false; }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 1_500);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [setNotice, browserRelayPairing, native, relayWorkflow]);

  const openBlockEditor: typeof gridWorkflow.openBlock = async (block) => {
    setParameterMenuInitiallyOpen(false);
    // Match CorOS: the Looper X block itself is the sole entry point to the
    // dedicated transport screen. Demo retains the parameter preview because
    // its virtual gateway deliberately has no looper transport.
    if (!demoActive && /\blooper\b/i.test(`${block.category ?? ""} ${block.name}`)) {
      await looperWorkflow.run("open");
      setMobileScreenView("looper-editor");
      return;
    }
    return gridWorkflow.openBlock(block);
  };

  const usbDiagnostics = async () => {
    if (!native) return "USB diagnostics are available in the installed Android app.";
    const runtimeDiagnostics = await androidUsbRuntimeDiagnostics();
    const result = runtimeDiagnostics.android;
    const traffic = `${result.messagesReceived} received / ${result.messagesSent} sent`;
    const last = result.lastMessageType >= 0 ? `; last message type ${result.lastMessageType}` : "";
    const errors = result.decodeErrors ? `; ${result.decodeErrors} decode error${result.decodeErrors === 1 ? "" : "s"}` : "";
    const stalls = result.expectedWriteStalls ? `; ${result.expectedWriteStalls} expected QC write stalls` : "";
    const catalog = result.modelCount ? `; ${result.modelCount} models loaded` : "";
    const stateAge = result.lastStateAt ? `; latest state ${Math.max(0, Date.now() - result.lastStateAt)} ms ago` : "";
    const endpoint = result.interfaceId >= 0 ? ` Interface ${result.interfaceId}, input endpoint 0x${result.inputEndpointAddress.toString(16)}, max packet ${result.inputMaxPacketSize}; ${result.readAttempts} read polls (${result.negativeReads} negative); ${result.reportBytes}-byte output framing.` : "";
    const midi = result.midiAvailable ? ` USB-MIDI interface ${result.midiInterfaceId}, output endpoint 0x${result.midiOutputEndpointAddress.toString(16)}; MIDI queue ${result.lastMidiQueueDelayMs} ms (max ${result.maxMidiQueueDelayMs} ms).` : " USB-MIDI output unavailable.";
    const problem = result.lastError ? ` Last error: ${result.lastError}` : "";
    return `USB ${result.connected ? "connected" : "not connected"}: ${traffic}${last}${errors}${stalls}${catalog}${stateAge}.${endpoint}${midi}${problem}`;
  };

  // This remains separate from connection recovery: reads still work while writes are paused.
  useEffect(() => {
    if (!native || !hardwareConnected) {
      // A suspension belongs to the session that reported it.
      setDeviceSuspended(false);
      return;
    }
    let cancelled = false;
    const poll = async () => {
      try {
        const runtimeDiagnostics = await androidUsbRuntimeDiagnostics();
        if (cancelled) return;
        // The shared owner's record is authoritative: it clears with the
        // session, so a lost resume frame cannot leave the controls disabled
        // after a reconnect.
        if (runtimeDiagnostics.suspension !== undefined) setDeviceSuspended(Boolean(runtimeDiagnostics.deviceSuspended));
        observeWriteHealth(Boolean(runtimeDiagnostics.writesQuarantined), runtimeDiagnostics.writeQuarantineReason);
      } catch { /* A failed diagnostics poll is not itself a quarantine signal. */ }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 2_000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [native, hardwareConnected, observeWriteHealth]);

  const startDemoDevice = async (reason?: string) => {
    if (!native || corpusFixtureEnabled || demoStartInFlight.current || demoActive) return;
    if (reason === undefined) usbRetryRequested.current = false;
    const attempt = connectionAttempts.begin();
    demoStartInFlight.current = true;
    setDemoPending(true);
    resetCommands();
    try {
      const opened = await demoDeviceSession.start(virtualDeviceStorage, attempt.current);
      if (!opened) return;
      const { connection: state, snapshot: current } = opened;
      activeSourceRef.current = "virtual";
      demoCanYieldToDevice.current = reason !== undefined;
      deviceHistory.clear();
      presetWorkflow.resetCache();
      setConnection(state);
      setSnapshot(current);
      setSelectedBlockId("");
      setMobileScreenView(null);
      setNotice(reason
        ? `Using the demo device because the Quad Cortex was not available (${reason}). ${current.presetLocation} · ${current.presetName} is saved on this phone.`
        : hardwareConnected
          ? `Demo device active. ${current.presetLocation} · ${current.presetName} is saved on this phone; the Quad Cortex stays connected in the background.`
          : `Demo device active. ${current.presetLocation} · ${current.presetName} is saved on this phone; nothing is sent to hardware.`);
    } catch (error) {
      if (!attempt.current()) return;
      const detail = error instanceof Error ? error.message : String(error);
      if (hardwareConnected) {
        demoDeviceSession.useDevice();
        activeSourceRef.current = "device";
        setNotice(`The demo device could not be opened: ${detail}`);
      } else {
        setConnection({ phase: "needs-attention", detail: `The demo device could not be opened: ${detail}`, demo: true, source: "device" });
        setNotice(`The demo device could not be opened: ${detail}`);
      }
    } finally {
      setDemoPending(false);
      demoStartInFlight.current = false;
    }
  };
  startDemoDeviceRef.current = startDemoDevice;

  const resumeHardwareDevice = async (knownConnected = false) => {
    if (!native || devicePending || demoPending || resumeHardwareInFlight.current) return;
    resumeHardwareInFlight.current = true;
    if (!hardwareConnected && !knownConnected) {
      try { await attemptUsbConnection(true); }
      finally { resumeHardwareInFlight.current = false; }
      return;
    }
    const attempt = connectionAttempts.begin();
    setDevicePending(true);
    try {
      const current = await androidGatewayTransport.currentSnapshot();
      if (!attempt.current()) return;
      demoDeviceSession.useDevice();
      activeSourceRef.current = "device";
      demoCanYieldToDevice.current = false;
      deviceHistory.clear();
      presetWorkflow.resetCache();
      setSnapshot(current);
      setSelectedBlockId("");
      setConnection({ phase: "ready", detail: "Quad Cortex connected.", demo: false, source: "device", lastSync: new Date().toISOString() });
      setNotice(`Quad Cortex active · ${current.presetLocation} · ${current.presetName}.`);
    } catch (error) {
      if (!attempt.current()) return;
      setHardwareConnected(false);
      setNotice(`The Quad Cortex connection was not available; the demo device remains active. ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setDevicePending(false);
      resumeHardwareInFlight.current = false;
    }
  };
  resumeHardwareDeviceRef.current = resumeHardwareDevice;

  const stopDemoDevice = () => {
    if (devicePending || demoPending) return;
    if (hardwareConnected) {
      void resumeHardwareDevice();
      return;
    }
    connectionAttempts.begin();
    demoDeviceSession.useDevice();
    activeSourceRef.current = "device";
    deviceHistory.clear();
    presetWorkflow.resetCache();
    resetCommands();
    setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
    setSelectedBlockId("");
    const idle = { phase: "disconnected" as const, detail: "Demo device stopped. Its presets stay saved on this phone.", demo: true, source: "device" as const };
    setConnection(idle);
    setNotice(idle.detail);
  };

  const copyDemoLibrary = async () => {
    const store = virtualDeviceStoreOf(demoDeviceSession.virtual);
    if (!store || demoDeviceSession.source !== "virtual") { setNotice("Start the demo device before copying its library."); return; }
    try {
      const library = store.state;
      const counts = demoLibraryCounts(library);
      await navigator.clipboard.writeText(serializeDemoLibrary(library));
      setNotice(`Copied ${counts.presets} demo preset${counts.presets === 1 ? "" : "s"} in ${counts.setlists} setlist${counts.setlists === 1 ? "" : "s"}.`);
    } catch (error) { setNotice(error instanceof Error ? error.message : "The demo library could not be copied."); }
  };

  const pasteDemoLibrary = async () => {
    const store = virtualDeviceStoreOf(demoDeviceSession.virtual);
    if (!store || demoDeviceSession.source !== "virtual") { setNotice("Start the demo device before pasting a library."); return; }
    try {
      const parsed = parseDemoLibraryText(await navigator.clipboard.readText());
      if (!parsed.ok) { setNotice(parsed.message); return; }
      const counts = demoLibraryCounts(parsed.document.library);
      if (!await appPrompts.confirm(`Replace the demo library on this phone with ${counts.presets} preset${counts.presets === 1 ? "" : "s"} in ${counts.setlists} setlist${counts.setlists === 1 ? "" : "s"}?`)) return;
      await store.replaceLibrary(parsed.document.library);
      const current = await activeGateway.currentSnapshot();
      deviceHistory.clear();
      presetWorkflow.resetCache();
      reconcileWorkflowSnapshot(current);
      setSelectedBlockId("");
      setNotice(`Demo library replaced: ${counts.presets} preset${counts.presets === 1 ? "" : "s"} in ${counts.setlists} setlist${counts.setlists === 1 ? "" : "s"}.`);
    } catch (error) { setNotice(error instanceof Error ? error.message : "The demo library could not be pasted."); }
  };

  const connectUsb = async () => {
    if (!native || usbBusy) return;
    await attemptUsbConnection(true);
  };
  const openUsbStatusPanel = async () => {
    setWorkflowPanel("usb");
    if (demoActive) {
      setUsbPanelDetail(hardwareConnected
        ? "The Quad Cortex remains connected in the background. Demo commands never reach it."
        : "No Quad Cortex is connected. Demo commands are saved on this phone only.");
      return;
    }
    try { setUsbPanelDetail(await usbDiagnostics()); }
    catch (error) { setUsbPanelDetail(error instanceof Error ? error.message : "USB status could not be read."); }
  };
  const openRemoteStatusPanel = async () => {
    setWorkflowPanel("remote");
    if (native) await relayWorkflow.refresh().catch(() => undefined);
    void refreshRelayUsage();
  };

  const adjustEditorParameter = useCallback((role: string, delta: number) => {
    return parameterWorkflow.adjustEncoder(role, delta, setNotice, false);
  }, [setNotice, parameterWorkflow]);
  const handleSurfaceAction = useQcSurfaceActions({
    snapshot,
    selectedBlockId,
    blockDetails,
    grid: gridWorkflow,
    performance: performanceWorkflow,
    openBlock: (block) => { void openBlockEditor(block); },
    closeBlock: () => { void gridWorkflow.closeOnDevice(); },
    // Ported from Windows: taps on the Grid's Splitter/Mixer marker or a
    // lane's Input Gate/Lane Output marker previously had no host handler on
    // Android at all (dispatchSurfaceCommand silently dropped them), so they
    // did nothing. These now open the same shared live editors Windows uses.
    openRoutingNode: (row, node) => { void gridWorkflow.openRoutingNode(row, node); },
    openLaneControl: (row, control) => { void gridWorkflow.openLaneControl(row, control); },
    openSplitPlacement: (row) => routingWorkflow.openSplitPlacement(row),
    rotation: { editor: adjustEditorParameter, tempo: continuousControls.adjustTempo, masterVolume: continuousControls.adjustMasterVolume },
    demo: demoActive,
    notice: setNotice
  });
  const ioViewOpen = Boolean(mobileScreenView?.startsWith("io-"));
  const gigViewOpen = Boolean(mobileScreenView?.startsWith("gig"));
  const cycleMode = () => {
    const current = snapshotRef.current;
    const slots = current.modeSlots?.filter((slot) => slot.mode) ?? [];
    if (!slots.length) return;
    const active = slots.findIndex((slot) => slot.mode === current.mode);
    const next = slots[(active + 1 + slots.length) % slots.length];
    void performanceWorkflow.selectModeSlot(next.slot);
  };
  const holdDelayMs = deviceSettingsWorkflow.settings?.holdTimingMs ?? 800;
  const controlTapSuppressed = (role: "bankDown" | "tempo") => Date.now() < suppressControlTapUntil.current[role];
  const pressSurfaceSwitch = (role: "bank:up" | "bank:down" | "tempo") => {
    handleSurfaceAction({ kind: "switch", role, phase: "press" });
    handleSurfaceAction({ kind: "switch", role, phase: "release" });
  };
  const handleModeChordContact = (role: "bankDown" | "tempo", pressed: boolean) => {
    const contacts = controlContacts.current;
    contacts[role] = pressed;
    const chord = modeChord.current;
    const nativeModeSurfaceVisible = !blockDetails && !deviceSettingsWorkflow.open && !tempoWorkflow.open
      && (mobileScreenView === null || mobileScreenView === "gig");
    if (contacts.bankDown && contacts.tempo && !chord.active && nativeModeSurfaceVisible) {
      chord.active = true;
      chord.held = false;
      suppressControlTapUntil.current = { bankDown: Number.POSITIVE_INFINITY, tempo: Number.POSITIVE_INFINITY };
      chord.timer = window.setTimeout(() => {
        chord.timer = undefined;
        if (!modeChord.current.active || !deviceSettingsWorkflow.settings?.gigViewStompAccess) return;
        modeChord.current.held = true;
        void deviceViews.toggle("gig");
      }, holdDelayMs);
      return;
    }
    if (!pressed && chord.active && chord.timer !== undefined) {
      if (chord.timer !== undefined) window.clearTimeout(chord.timer);
      chord.timer = undefined;
    }
    if (!contacts.bankDown && !contacts.tempo && chord.active) {
      suppressControlTapUntil.current = { bankDown: Date.now() + 400, tempo: Date.now() + 400 };
      const held = chord.held;
      chord.active = false;
      chord.held = false;
      if (!held) cycleMode();
    }
  };
  const openTempoOrTuner = (gesture: "hold" | "double-tap") => {
    if (modeChord.current.active || blockDetails || deviceSettingsWorkflow.open || tempoWorkflow.open || mobileScreenView !== null) return;
    const swapped = Boolean(deviceSettingsWorkflow.settings?.swapTempoTunerAccess);
    const openTuner = gesture === (swapped ? "double-tap" : "hold");
    if (openTuner) { setWorkflowPanel(null); void deviceViews.open("tuner"); }
    else tempoWorkflow.openTempo();
  };
  const display = createQcDeviceDisplay({
    snapshot, selectedBlockId, pending: devicePending, destination: screenDestination, views: deviceViews, cpu: cpuTelemetry, undo: deviceUndo,
    workflows: { deviceSettings: deviceSettingsWorkflow, tempo: tempoWorkflow, tuner: tunerWorkflow, looper: looperWorkflow, modes: modesWorkflow, midiOut: midiOutWorkflow, ioEq: ioEqWorkflow, grid: gridWorkflow, scene: sceneWorkflow, preset: presetWorkflow, routing: routingWorkflow },
    mirror: activeGateway,
    selectScene: performanceWorkflow.selectScene, queueTempo: continuousControls.queueTempo, setParameterMenuInitiallyOpen, notice: setNotice,
    expressionAssignment, laneControlEditor, parameterEditor: parameterEditorBindings,
    openPreset: () => void presetWorkflow.openDirectory(), refresh: () => void presetWorkflow.refresh(), openRouting: routingWorkflow.openPicker,
    fixtureScreenView,
    auditContent: productionAuditScreen ? <CorOsProductionScreenAudit view={productionAuditScreen} snapshot={snapshot} /> : undefined
  });
  // The unit's own switches below the display light with the open parameter editor.
  const parameterLeds = display.hardwareLeds;
  const mobileLed = (slot: number, fallback: { active: boolean; assigned: boolean; color: string }) => parameterLeds?.[slot] ?? fallback;

  const configureRelay = async () => {
    if (!native) return;
    if (relayPaired) {
      if (!window.confirm("Unpair this phone from the remote QC relay?")) return;
      const status = await relayWorkflow.unpair();
      if (status?.detail) window.alert(status.detail);
      return;
    }
    setBrowserRelayPairingPending(true);
    try {
      const pairing = await publicRelay.browserPairing!.begin();
      setBrowserRelayPairing({ expiresAt: pairing.expiresAt });
      setNotice("Browser pairing opened. Sign in there, then return to QC Remote; pairing will finish automatically.");
    } catch (error) { setNotice(error instanceof Error ? error.message : "Relay pairing failed."); }
    finally { setBrowserRelayPairingPending(false); }
  };

  const changeControlAccessMode = async (mode: ControlAccessMode) => {
    const previous = controlAccessMode;
    setControlAccessMode(mode);
    writeControlAccessMode(window.localStorage, mode);
    if (!native) return;
    try {
      await relayWorkflow.setAccessMode(mode);
      setNotice(`MCP control access changed to ${mode}. Guarded confirmations still apply; manual controls remain available.`);
    } catch (error) {
      setControlAccessMode(previous);
      writeControlAccessMode(window.localStorage, previous);
      setNotice(error instanceof Error ? error.message : "Could not change the remote access mode.");
    }
  };

  return <main className={`android-app${screenDimmed ? " screen-dimmed" : ""}`} onPointerDown={resetScreenDimmer} onKeyDown={resetScreenDimmer} onTouchStart={resetScreenDimmer}>
    <header className="mobile-header">
      <div className="mobile-brand"><AppMark onClick={() => setWorkflowPanel("about")} /><span><strong>{QC_BRAND.appName}</strong><small>v{appPackage.version}</small></span></div>
      <div className="connection-pills">
        <button className={`connection-pill phase-${remotePresentation.phase}`} onClick={() => void openRemoteStatusPanel()} aria-haspopup="dialog" aria-expanded={workflowPanel === "remote"} aria-label={`REMOTE ${remotePresentation.state}; open remote access details`}><i /> REMOTE</button>
        <button className={`connection-pill phase-${usbIndicatorPhase}`} onClick={() => void openUsbStatusPanel()} aria-haspopup="dialog" aria-expanded={workflowPanel === "usb"} aria-label={`${demoActive ? "DEMO" : "USB"}; open device connection details`}><i /> {demoActive ? "DEMO" : "USB"}</button>
      </div>
    </header>

    <section className="mobile-screen" aria-label="Quad Cortex display">
      <QcDeviceDisplay display={display} presentation="display" formFactor={formFactor} skin={skin} onAction={handleSurfaceAction} onMasterVolumeInteractionStart={continuousControls.beginMasterVolumeInteraction} onMasterVolumeInteractionEnd={continuousControls.endMasterVolumeInteraction} />
    </section>

    <nav className="quick-controls" aria-label="Quick device controls">
      <div className="mobile-volume-control"><QcMasterVolumeKnob value={snapshot.masterVolume} readout={`${snapshot.masterVolume}`} onAction={handleSurfaceAction} onInteractionStart={continuousControls.beginMasterVolumeInteraction} onInteractionEnd={continuousControls.endMasterVolumeInteraction} /></div>
      <button type="button" className="mobile-device-view-control mobile-io-control" aria-label={ioViewOpen ? "I/O Settings, close view" : "Open I/O Settings"} aria-pressed={ioViewOpen} onClick={() => void deviceViews.toggle("io")}>I/O</button>
      <button type="button" className="mobile-device-view-control mobile-gig-control" aria-label={gigViewOpen ? "Gig View, close view" : "Open Gig View"} aria-pressed={gigViewOpen} onClick={() => void deviceViews.toggle("gig")}>GIG</button>
      <div className="mobile-up-control"><QcHardwareSwitch role="bank:up" label={<QcUiIcon kind="up" />} ariaLabel="Previous preset" active={Boolean(parameterEditorBindings)} assigned={Boolean(parameterEditorBindings)} accent={QC_COLORS.hardware.whiteLed} onTap={() => {
        if (mobileScreenView === "tuner") void deviceViews.close();
        else if (tempoWorkflow.open) tempoWorkflow.close();
        else pressSurfaceSwitch("bank:up");
      }} onAction={handleSurfaceAction} /></div>
      {sceneFootswitches.map(({ index, label }) => {
        const slot = index < 4 ? index : index + 1;
        const led = mobileLed(slot, switchLeds[index]);
        return <div className={`mobile-encoder-control mobile-encoder-${index}`} key={label}>
          <QcHardwareSwitch role={`footswitch:${label}`} label={label} active={led.active} assigned={led.assigned} accent={led.color} onAction={handleSurfaceAction} />
        </div>;
      })}
      {(() => { const led = mobileLed(4, { active: false, assigned: false, color: QC_COLORS.hardware.whiteLed }); return <div className="mobile-down-control"><QcHardwareSwitch role="bank:down" label={<span className="mobile-down-glyph"><QcUiIcon kind="up" /></span>} ariaLabel="Next preset" active={led.active} assigned={led.assigned} accent={led.color} onContactChange={(pressed) => handleModeChordContact("bankDown", pressed)} onTap={() => { if (!controlTapSuppressed("bankDown")) pressSurfaceSwitch("bank:down"); }} onAction={handleSurfaceAction} /></div>; })()}
      <div className="mobile-tempo-control"><QcHardwareSwitch role="tempo" label="" ariaLabel="TEMPO" readout={`${snapshot.tempo}`} active={parameterLeds ? parameterLeds[9].active : snapshot.tempoLedEnabled} assigned={parameterLeds ? parameterLeds[9].assigned : snapshot.tempoLedEnabled} accent={parameterLeds ? parameterLeds[9].color : QC_COLORS.device.tempoLed} pulseBpm={!parameterLeds && snapshot.tempoLedEnabled ? snapshot.tempo : undefined} pulseEpochMs={!parameterLeds ? snapshot.tempoPulseEpochMs : undefined} holdDelayMs={holdDelayMs} onContactChange={(pressed) => handleModeChordContact("tempo", pressed)} onTap={() => { if (!controlTapSuppressed("tempo")) pressSurfaceSwitch("tempo"); }} onDoubleTap={() => openTempoOrTuner("double-tap")} onHold={() => openTempoOrTuner("hold")} onAction={handleSurfaceAction} /></div>
    </nav>
    <div className="mobile-status-notice" role="status">{notice}</div>


    {workflowPanel && <div className="mobile-workflow-backdrop" role="presentation" onClick={() => setWorkflowPanel(null)}>
      <section className="mobile-workflow-panel" role="dialog" aria-modal="true" aria-labelledby="dialog-title" onClick={(event) => event.stopPropagation()}>
        <button className="mobile-workflow-close" aria-label="Close" onClick={() => setWorkflowPanel(null)}><QcUiIcon kind="close" /></button>
        {workflowPanel === "usb" && <div className={`mobile-connection-panel${demoActive ? " is-demo" : ""}`}>
          <div className="dialog-kicker">DEVICE CONNECTION</div><h2 id="dialog-title">{demoActive ? "Demo device" : "Quad Cortex"}</h2>
          <div className="mobile-device-tabs" role="group" aria-label="Active device">
            <button type="button" aria-pressed={!demoActive} disabled={!native || usbBusy || devicePending || demoPending} onClick={() => void (demoActive ? resumeHardwareDevice() : openUsbStatusPanel())}><i className={`panel-status-dot phase-${demoActive ? hardwareConnected ? "ready" : "idle" : usbIndicatorPhase}`} />Quad Cortex<small>USB</small></button>
            <button type="button" className="demo-device-tab" aria-pressed={demoActive} disabled={!native || usbBusy || devicePending || demoPending} onClick={() => { if (!demoActive) void startDemoDevice(); }}>DEMO{demoPending && <small>Opening…</small>}</button>
          </div>
          <div className="mobile-connection-summary"><span><i className={`panel-status-dot phase-${demoActive ? "demo" : usbIndicatorPhase}`} />{demoActive ? "Demo device ready" : usbStatusText}</span>{!demoActive && <button type="button" disabled={!native || usbBusy} onClick={() => void connectUsb().then(openUsbStatusPanel)}>{hardwareConnected ? "Reconnect" : "Connect"}</button>}{demoActive && !hardwareConnected && <button type="button" disabled={!native || usbBusy || devicePending || demoPending} onClick={() => void resumeHardwareDevice()}>Connect Quad Cortex</button>}</div>
          <dl className="mobile-connection-details">
            <div><dt>Connection</dt><dd>{demoActive ? "Local simulation · no hardware changes" : connection.detail || (hardwareConnected ? "The Quad Cortex is connected directly over USB." : "No Quad Cortex is connected directly over USB.")}</dd></div>
            <div><dt>Current preset</dt><dd>{demoActive ? `${snapshot.presetLocation} · ${snapshot.presetName}` : hardwareConnected ? `${snapshot.presetLocation} · ${snapshot.presetName}` : "Not synchronized"}</dd></div>
            {!demoActive && snapshot.gridLayoutComplete === false && <div><dt>Grid</dt><dd role="status">{gridRefreshPending ? "Refreshing the current preset… Grid editing is paused." : "Grid layout is incomplete. Grid editing is paused until a fresh current-preset read succeeds."}</dd></div>}
            <div><dt>Status</dt><dd>{usbPanelDetail}</dd></div>
          </dl>
          <div className="mobile-panel-actions"><button disabled={!sessionReady || devicePending || demoPending} onClick={() => { setWorkflowPanel(null); void deviceViews.open("info"); }}>Device information</button></div>
          {demoActive && <div className="mobile-panel-actions"><button disabled={devicePending || demoPending} onClick={() => void copyDemoLibrary()}>Copy library</button><button disabled={devicePending || demoPending} onClick={() => void pasteDemoLibrary()}>Paste library</button><button className="danger" disabled={devicePending || demoPending} onClick={stopDemoDevice}>Stop demo</button></div>}
        </div>}
        {workflowPanel === "remote" && <div className="mobile-connection-panel mobile-remote-panel">
          <div className="dialog-kicker">REMOTE MCP ACCESS</div><h2 id="dialog-title">Quad Cortex pairing</h2>
          <div className="mobile-connection-summary"><span><i className={`panel-status-dot phase-${remotePresentation.phase}`} />{remotePresentation.state}</span><ControlAccessSelect value={controlAccessMode} ariaLabel="MCP device access" disabled={devicePending} onChange={(mode) => void changeControlAccessMode(mode)} /></div>
          <p className="mobile-panel-description">{remotePresentation.detail}</p>
          <dl className="mobile-connection-details">
            <div><dt>Pairing</dt><dd>{relayPaired ? relayWorkflow.status?.pairing?.email ?? "Paired" : browserRelayPairing ? "Waiting for browser sign-in" : "Not paired"}</dd></div>
            <div><dt>Tunnel</dt><dd>{relayWorkflow.status?.state === "connected" ? "Connected" : relayWorkflow.status?.state?.replaceAll("_", " ") ?? "Checking"}</dd></div>
            <div><dt>Control access</dt><dd>{controlAccessMode}</dd></div>
          </dl>
          {browserRelayPairing && <p className="mobile-status-detail">Browser sign-in expires {new Date(browserRelayPairing.expiresAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</p>}
          {relayWorkflow.status?.endpoint && <p className="mobile-status-detail">Remote service: {relayWorkflow.status.endpoint}</p>}
          {relayUsage && <p className="mobile-status-detail">Today: {relayUsage.commands} remote command{relayUsage.commands === 1 ? "" : "s"}{relayUsage.lastCommandAt ? ` · last command ${new Date(relayUsage.lastCommandAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}` : " · no remote command yet"}</p>}
          <p className="mobile-panel-footnote">Encrypted outbound connection · no inbound port is opened. On Android 7–9, native backups use app-private storage and are removed if QC Remote is uninstalled.</p>
          <div className="mobile-panel-actions"><button className="primary mobile-primary-action" disabled={!native || relayWorkflow.pending || browserRelayPairingPending || Boolean(browserRelayPairing) || remotePresentation.action === "none" || remotePresentation.action === "checking"} onClick={() => void (remotePresentation.action === "reconnect" ? startRelayReportingFailure(relayWorkflow.start, setNotice) : configureRelay())}>{remotePresentation.primaryAction}</button><button aria-label="Refresh remote status" title="Refresh" disabled={!native || relayWorkflow.pending} onClick={() => { void relayWorkflow.refresh(); void refreshRelayUsage(); }}><QcUiIcon kind="refresh" /></button>{relayPaired && <button className="danger" disabled={relayWorkflow.pending} onClick={() => void configureRelay()}>Unpair</button>}</div>
          <RemoteAccountDeletionLink disabled={!native} onOpen={() => {
            void openRemoteAccountDeletionPage().catch(() => setNotice("Could not open the account deletion page. Try again when a browser is available."));
          }} />
        </div>}
        {workflowPanel === "about" && <><div className="dialog-kicker">ABOUT</div><h2 id="dialog-title">{QC_BRAND.appName} <small>{appPackage.version}</small></h2><p>An independent mobile companion for controlling a connected Quad Cortex.</p><p className="mobile-legal-note">{QC_LEGAL.independence}</p><p>{QC_LEGAL.copyright}</p><div className="mobile-legal-actions"><button onClick={() => setWorkflowPanel("display")}>Display</button><button onClick={() => setWorkflowPanel("privacy")}>Privacy</button><button onClick={() => setWorkflowPanel("legal")}>Legal</button><button onClick={() => setWorkflowPanel("notices")}>Notices</button></div></>}
        {workflowPanel === "display" && <><div className="dialog-kicker">DISPLAY</div><h2 id="dialog-title">Display behavior</h2><p>Keep this screen on while QC Remote is in front and a Quad Cortex is connected. It automatically releases when you disconnect or leave the app.</p><label className="mobile-setting-toggle"><input type="checkbox" checked={keepScreenAwake} onChange={(event) => changeKeepScreenAwake(event.target.checked)} /> Keep screen awake while connected</label><label className="mobile-setting-toggle"><input type="checkbox" checked={followQcScreen} onChange={(event) => changeFollowQcScreen(event.target.checked)} /> Follow QC screen</label><p>When enabled, QC Remote checks the device screen adaptively while this app is visible. It pauses while hidden or disconnected.</p><div className="mobile-legal-actions"><button onClick={() => setWorkflowPanel("about")}>Back to About</button></div></>}
        {workflowPanel === "privacy" && <><div className="dialog-kicker">PRIVACY</div><h2 id="dialog-title">Privacy policy</h2><p>{QC_LEGAL.privacy.pointer}</p>{QC_BRAND.privacyPolicyUrl && <p><a href={QC_BRAND.privacyPolicyUrl} target="_blank" rel="noreferrer">Open the complete privacy policy</a></p>}<div className="mobile-legal-actions"><button onClick={() => setWorkflowPanel("about")}>Back to About</button></div></>}
        {workflowPanel === "legal" && <><div className="dialog-kicker">LEGAL</div><h2 id="dialog-title">Independent companion</h2><p>{QC_LEGAL.independence}</p><p>{QC_LEGAL.trademarks}</p><p>{QC_LEGAL.productSafety}</p><p>{QC_LEGAL.copyright}</p><p>{QC_LEGAL.license}</p>{QC_LEGAL.publisher.website && <p><a href={QC_LEGAL.publisher.website} target="_blank" rel="noreferrer">Publisher website</a></p>}{QC_BRAND.termsUrl && <p><a href={QC_BRAND.termsUrl} target="_blank" rel="noreferrer">Open the complete terms of use</a></p>}{QC_LEGAL.publisher.securityPolicyUrl && <p><a href={QC_LEGAL.publisher.securityPolicyUrl} target="_blank" rel="noreferrer">Open the security policy</a></p>}<div className="mobile-legal-actions"><button onClick={() => setWorkflowPanel("about")}>Back to About</button></div></>}
        {workflowPanel === "notices" && <><div className="dialog-kicker">THIRD-PARTY NOTICES</div><h2 id="dialog-title">Runtime components</h2>{QC_LEGAL.thirdParty.map((notice) => <p key={notice}>{notice}</p>)}{QC_LEGAL_DOCUMENTS.map((legalDocument) => <p key={legalDocument.id}><a href={`./legal/${legalDocument.bundledName}`} target="_blank" rel="noreferrer">{legalDocument.openLabel}</a></p>)}<div className="mobile-legal-actions"><button onClick={() => setWorkflowPanel("about")}>Back to About</button></div></>}
      </section>
    </div>}

    {/* Copy/swap remains the Grid context-menu command. Scene label/color is
        the current device screen, reached from Gig View's scene tile. */}
    <QcDeviceDisplayOverlay display={display} />
    {textPromptOverlay}
  </main>;
}
