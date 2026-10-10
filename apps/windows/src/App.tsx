import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type ConnectionState, type CpuColumnLoad, type DeviceActionResult, type DiagnosticsReport, type PresetSnapshot, type RuntimeStatus, type WorkspaceDocument } from "@qc-remote/client";
import { demoBlockDetails, demoSnapshot, sceneLetter, SupersededPresetOperation, saveWorkspaceDocument, type ControlAccessMode, type PublicRelayConnector } from "@qc-remote/core";
import { formFactors, skins } from "@qc-remote/form-factors";
import { QC_BRAND } from "@qc-remote/theme";
import { CONTROL_ACCESS_OPTIONS, CONNECTING_PLACEHOLDER_SNAPSHOT, corosFixtureConfiguration, createGridRepairCoordinator, refreshGridSnapshotOnce, CorOsCpuMonitor, CorOsDeviceSettings, CorOsGlobalEqScreen, CorOsIoSettingsScreen, CorOsSceneCommand, CorOsTempoScreen, CorOsTunerScreen, corOsUnavailableContextActionMessage, createDemoDeviceSession, IDLE_PLACEHOLDER_SNAPSHOT, LooperXScreen, ModesConfigurationScreen, PresetMidiOutEditor, usePresetMidiOutWorkflow, qcLaneControlPanelBindings, qcLiveIndicator, qcParameterEditorBindings, QuadCortexSurface, readControlAccessMode, readQcScreenFollowSetting, useAdaptiveQcScreenPolling, useBlockEditorSession, useContinuousControlWorkflow, useDeviceSettingsWorkflow, useExpressionAssignment, useIoEqWorkflows, useLooperXWorkflow, useModesConfigurationWorkflow, useNativeStateRecovery, usePublicRelayWorkflow, useQcConnectionWorkflow, useQcController, useQcLiveState, useQcSurfaceActions, useQcWorkflows, useTempoWorkflow, useTunerWorkflow, useWriteHealth, writeControlAccessMode, writeQcScreenFollowSetting, capturePresetText, captureSetlistForDemo, importCapturedSetlistIntoDemo, isGridDevice, isIncompleteGridRefreshFailure, parsePresetText, pastePresetIntoDemo, presetReplayConfirmation, presetReplaySummary, refreshGridSnapshot, replayPresetOnDevice, virtualDeviceStoreOf, type CapturedSetlist, type CorOsContextAction, type CorOsScreenView, type QcScreenObservation } from "@qc-remote/ui";
import { CorOsProductionScreenAudit } from "@qc-remote/ui/dev";
import { appWindows, brokerDevice, DEVICE_INFO_EVENTS, diagnosticsFiles, externalLinks, helpWindows, localAccess, localClientPackages, physicalDevices, type PhysicalQcDevice, onDevicePhase, publicRelay, publishWindowState, serveAppWindowRequests, serveDiagnosticsRequests, SETTINGS_EVENTS, tauriTransport, virtualDeviceStorage, WORKSPACE_EVENTS, workspaceFiles, type AntigravityLocalIntegrationStatus, type ChatGptLocalIntegrationStatus, type ClaudeDesktopExtensionStatus, type JanLocalIntegrationStatus, type LmStudioLocalIntegrationStatus, type LocalAccessStatus, type OtherMcpStdioSetup } from "./tauri-transport";
import { createWindowsQcTransport } from "./qc-transport";
import { useWindowsDeviceFrames } from "./use-windows-device-frames";
import { useWindowsPrompts } from "./use-windows-prompts";
import { divider, MenuBar, type AppMenu, type ConnectionEvent, type MenuCommand, type MenuItem } from "./menu-bar";
import { antigravityIntegrationPresentation, BIONIC_APP_NAME, chatgptIntegrationPresentation, claudeExtensionPresentation, lmStudioAppName, lmStudioEnableSwitch, localAccessNotice, localActionFailure, localPairOutcome, managedMcpIntegrationPresentation, type LocalActionOutcome } from "./local-integration-presentation";
import { brokerPhaseDetectsDevice, isQcConnectionFailure, qcConnectionDuringBrokerPhase, qcConnectionIsVerified, qcShouldConnectOnDetection, qcShouldDisconnectForDemo, qcVirtualSessionIsReady, qcVisibleStatusNotice, runtimeHasSynchronizedQc, syncProgressAt, syncProgressFinishMs } from "./qc-readiness";
import { DEFAULT_BROKER_TRAY_STATUS, type BrokerTrayStatus, type DeviceInfoState, type SettingsState, type WorkspaceState } from "./app-window-content";
import { brokerTray, changeBrokerTray } from "./tauri-transport";
import appPackage from "../package.json";
import QRCode from "qrcode";
import { qcLiveSessionCanObserveState, qcStreamRestartClearsBulkOperation } from "@qc-remote/ui";

const { enabled: corpusFixtureEnabled, screenView: fixtureScreenView, initialSnapshot: fixtureInitialSnapshot } =
  corosFixtureConfiguration(import.meta.env.DEV ? window.location.search : "", demoSnapshot);
const productionAuditScreen = import.meta.env.DEV ? new URLSearchParams(window.location.search).get("productionScreen") : null;

const initialConnection: ConnectionState = {
  phase: "disconnected",
  detail: "Device gateway is not connected",
  demo: true,
  source: "device"
};

const storedAccessMode = (): ControlAccessMode => readControlAccessMode(localStorage);
const appVersion = appPackage.version;
const publicRelayEndpoint = QC_BRAND.publicRelayEndpoint;
export function App() {
  const { connection, setConnection, attempts: connectionAttempts } = useQcConnectionWorkflow(initialConnection);
  const [syncProgress, setSyncProgress] = useState<number | null>(null);
  const [runtime, setRuntime] = useState<RuntimeStatus>();
  const [deviceSuspended, setDeviceSuspended] = useState(false);
  const [deviceBulkBlocking, setDeviceBulkBlocking] = useState(false);
  const [catalogRevision, setCatalogRevision] = useState(0);
  // `deviceReady` is real, verified hardware and nothing else: it drives every
  // live-hardware surface and all "Quad Cortex" wording. `sessionReady` also
  // admits the demo device, and gates only whether commands can be issued.
  const deviceReady = qcConnectionIsVerified(connection, runtime, syncProgress) && !deviceSuspended && !deviceBulkBlocking;
  const virtualReady = qcVirtualSessionIsReady(connection);
  const sessionReady = deviceReady || virtualReady;
  const qcController = useQcController(demoSnapshot);
  const {
    snapshot, snapshotRef, setSnapshot,
    resetCommands, reconcileFrame
  } = qcController;
  useEffect(() => { if (corpusFixtureEnabled) setSnapshot(fixtureInitialSnapshot); }, [setSnapshot]);
  const [selectedBlockId, setSelectedBlockId] = useState("");
  const [formFactorId, setFormFactorId] = useState("quad-cortex-large");
  const [notice, setNotice] = useState("Demo state loaded. Connect the device gateway to enable hardware commands.");
  const [gridRefreshPending, setGridRefreshPending] = useState(false);
  const { observe: observeWriteHealth } = useWriteHealth({
    enabled: Boolean(window.__TAURI_INTERNALS__) && !corpusFixtureEnabled,
    recoveryInstruction: "the session is reset",
    onNotice: setNotice
  });
  const [commandPending, setCommandPending] = useState(false);
  const { prompts: appPrompts, overlay: textPromptOverlay } = useWindowsPrompts();
  const [controlAccessMode, setControlAccessMode] = useState<ControlAccessMode>(storedAccessMode);
  const [followQcScreen, setFollowQcScreen] = useState(() => readQcScreenFollowSetting(localStorage));
  const [relayBrowserPairing, setRelayBrowserPairing] = useState<{ pairUrl: string; expiresAt: number; qrCode: string }>();
  const [relayPairingStarting, setRelayPairingStarting] = useState(false);
  const [relayPairingError, setRelayPairingError] = useState<string>();
  const [relayConnectors, setRelayConnectors] = useState<PublicRelayConnector[]>([]);
  const [relayConnectorsError, setRelayConnectorsError] = useState<string>();
  const [relayUnlinkingGrantId, setRelayUnlinkingGrantId] = useState<string>();
  // Whether REMOTE is on screen. A pairing code is created when it opens
  // and withdrawn when it closes, so one never outlives the panel that
  // displays it, and a pairing that completes while it is open is followed
  // by a fresh code rather than by an empty space.
  const relayPanelOpen = useRef(false);
  const [claudeExtensionStatus, setClaudeExtensionStatus] = useState<ClaudeDesktopExtensionStatus>();
  const [claudeInstallPending, setClaudeInstallPending] = useState(false);
  const [chatgptIntegrationStatus, setChatgptIntegrationStatus] = useState<ChatGptLocalIntegrationStatus>();
  const [chatgptRestartRequired, setChatgptRestartRequired] = useState(false);
  const [chatgptRemovalRestartRequired, setChatgptRemovalRestartRequired] = useState(false);
  const [antigravityIntegrationStatus, setAntigravityIntegrationStatus] = useState<AntigravityLocalIntegrationStatus>();
  const [lmStudioIntegrationStatus, setLmStudioIntegrationStatus] = useState<LmStudioLocalIntegrationStatus>();
  const [janIntegrationStatus, setJanIntegrationStatus] = useState<JanLocalIntegrationStatus>();
  const [otherMcpStatus, setOtherMcpStatus] = useState<OtherMcpStdioSetup>();
  const [localStatusChecking, setLocalStatusChecking] = useState(false);
  const [claudeStatusRefreshFailed, setClaudeStatusRefreshFailed] = useState(false);
  const [chatgptStatusRefreshFailed, setChatgptStatusRefreshFailed] = useState(false);
  const [antigravityStatusRefreshFailed, setAntigravityStatusRefreshFailed] = useState(false);
  const [lmStudioStatusRefreshFailed, setLmStudioStatusRefreshFailed] = useState(false);
  const [janStatusRefreshFailed, setJanStatusRefreshFailed] = useState(false);
  const [otherMcpStatusRefreshFailed, setOtherMcpStatusRefreshFailed] = useState(false);
  const [localPanelOpen, setLocalPanelOpen] = useState(false);
  const [localIntegrationAction, setLocalIntegrationAction] = useState<"claude" | "chatgpt" | "remove-chatgpt" | "antigravity" | "remove-antigravity" | "lm-studio" | "remove-lm-studio" | "jan" | "remove-jan" | "other" | null>(null);
  // Every LOCAL result is reported twice: once inside the provider card the
  // user is looking at, and once in the status strip that mirrors it.
  const [localAccessStatus, setLocalAccessStatus] = useState<LocalAccessStatus>();
  const [claudeApprovalLapsed, setClaudeApprovalLapsed] = useState(false);
  const localStatusRefreshInFlight = useRef(false);
  const editor = useBlockEditorSession();
  const { details: blockDetails } = editor;
  const [parameterMenuInitiallyOpen, setParameterMenuInitiallyOpen] = useState(false);
  const [workspacePath, setWorkspacePath] = useState<string>();
  const [workspaceName, setWorkspaceName] = useState<string>();
  const [loadedWorkspace, setLoadedWorkspace] = useState<WorkspaceDocument>();
  const [surfaceView, setSurfaceView] = useState<"fit" | "actual">("fit");
  const gridRefreshRequest = useRef(0);
  const gridRepair = useRef(createGridRepairCoordinator());
  const [deviceScreenViewState, setScreenDestination] = useState<{ view: CorOsScreenView | null; returnFocusTarget?: "grid-menu" }>({ view: null });
  const deviceScreenView = deviceScreenViewState.view;
  const setDeviceScreenView = useCallback((next: CorOsScreenView | null | ((current: CorOsScreenView | null) => CorOsScreenView | null)) => {
    setScreenDestination((current) => {
      const view = typeof next === "function" ? next(current.view) : next;
      // Preserve a functional no-op from an unrelated gig-state publication.
      if (typeof next === "function" && view === current.view) return current;
      return { view };
    });
  }, []);
  const [cpuLoad, setCpuLoad] = useState(0);
  const [cpuColumns, setCpuColumns] = useState<CpuColumnLoad[]>([]);
  const [deviceUndoAvailable, setDeviceUndoAvailable] = useState(false);
  const [deviceRedoAvailable, setDeviceRedoAvailable] = useState(false);
  const consumeLiveState = useQcLiveState({
    reconcileFrame,
    editor,
    onStates: (states, frame) => {
      for (const state of states) {
        if (state.kind === "gigView" && state.gigViewShown !== undefined) {
          setDeviceScreenView((current) => state.gigViewShown ? "gig" : current?.startsWith("gig") ? null : current);
        } else if (state.kind === "gridTopology" && state.gridLayoutComplete === false
            && !connection.demo && connection.source !== "virtual") {
          // The sparse echo does not identify a block well enough to update
          // the displayed topology. Ask the explicit current-preset path to
          // refresh once; ordinary cached snapshots remain frame-free, and
          // this repair is bounded to one request for a complete publication.
          editor.close();
          setSelectedBlockId("");
          const attempt = connectionAttempts.capture();
          void refreshGridSnapshotOnce(gridRepair.current, frame?.streamId ?? nativeStateStream.current, frame?.sequence, {
            sequence: gridRefreshRequest,
            readSnapshot: qcController.readSnapshot,
            read: () => tauriTransport.currentPreset(),
            isCurrent: () => attempt.current()
              && connectionRef.current.source === "device" && !connectionRef.current.demo,
            onPending: () => setGridRefreshPending(true),
            onError: (error) => setNotice(error instanceof Error ? error.message : "The Grid layout is incomplete; current preset could not be refreshed."),
            onFinished: () => setGridRefreshPending(false)
          });
        } else if (state.kind === "cpuLoad") {
          if (state.cpuTotalLoad !== undefined) setCpuLoad(state.cpuTotalLoad);
          if (state.cpuColumns) setCpuColumns(state.cpuColumns);
        } else if (state.kind === "history") {
          if (state.undoAvailable !== undefined) setDeviceUndoAvailable(state.undoAvailable);
          if (state.redoAvailable !== undefined) setDeviceRedoAvailable(state.redoAvailable);
        } else if (state.kind === "suspend" && state.suspended !== undefined) {
          setDeviceSuspended(state.suspended);
          setNotice(state.suspended ? `Quad Cortex temporarily suspended control${state.suspendReason ? ` (${state.suspendReason.replaceAll("_", " ").toLowerCase()})` : ""}.` : "Quad Cortex control resumed.");
        } else if (state.kind === "refresh" && state.refreshRequired) {
          void qcController.readSnapshot(() => tauriTransport.currentSnapshot(), { sessionRecovery: true }).catch(() => undefined);
        } else if (state.kind === "catalog" && state.catalogRefresh) {
          setCatalogRevision((revision) => revision + 1);
          setNotice("The Quad Cortex model catalog changed. Add Device will reload it when opened.");
        } else if (state.kind === "bulkOperation" && state.bulkOperation) {
          const blocking = Boolean(state.bulkOperation.blocking && !state.bulkOperation.finished);
          setDeviceBulkBlocking(blocking);
          if (state.bulkOperation.message) setNotice(state.bulkOperation.message);
        }
      }
    }
  });
  const [deviceInformation, setDeviceInformation] = useState<{ serial?: string; deviceName?: string; corOsVersion?: string }>();
  const [setlistClipboard, setSetlistClipboard] = useState<{ serial: string; source: "device" | "virtual"; setlistKey: string; setlistName: string; captured?: CapturedSetlist }>();
  const [fullScreen, setFullScreen] = useState(Boolean(document.fullscreenElement));
  const [appMenuOpen, setAppMenuOpen] = useState(false);
  const [connectionEvents, setConnectionEvents] = useState<ConnectionEvent[]>([{ at: new Date().toISOString(), event: "app-start", result: "info", detail: `${QC_BRAND.appName} started; waiting for the desktop runtime.` }]);
  const autoConnectStarted = useRef(false);
  const liveSyncFailures = useRef(0);
  const [physicalQcDevices, setPhysicalQcDevices] = useState<PhysicalQcDevice[]>([]);
  const [selectedPhysicalId, setSelectedPhysicalId] = useState<string>();
  const selectedPhysicalRef = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    let disposed = false;
    let timer: number | undefined;
    const refresh = async () => {
      try {
        const inventory = await physicalDevices.list();
        if (disposed) return;
        setPhysicalQcDevices((previous) => {
          const selected = selectedPhysicalRef.current;
          if (!selected || inventory.devices.some((device) => device.id === selected)) return inventory.devices;
          const last = previous.find((device) => device.id === selected);
          return [...inventory.devices, {id: selected, name: last?.name ?? "Quad Cortex", connected: false, phase: "disconnected"}];
        });
        if (!selectedPhysicalRef.current && inventory.selectedDeviceId) {
          selectedPhysicalRef.current = inventory.selectedDeviceId;
          setSelectedPhysicalId(inventory.selectedDeviceId);
        }
      } catch { /* Preserve the last inventory during broker recovery. */ }
      finally { if (!disposed) timer = window.setTimeout(() => void refresh(), 2000); }
    };
    void refresh();
    return () => { disposed = true; if (timer !== undefined) window.clearTimeout(timer); };
  }, []);
  const nativeStateSequence = useRef(0);
  const nativeStateStream = useRef<string | undefined>(undefined);
  const nativeStateRecoveryRevision = useRef(0);
  const pendingNativeStateRecovery = useRef<{revision: number; streamId: string | undefined; reason: "restarted" | "gap"} | undefined>(undefined);
  const reportedNativeStateRecoveryFailure = useRef<number | undefined>(undefined);
  const nativeStateAvailable = useRef(false);
  const relayPairingStartInFlight = useRef(false);
  const syncProgressTimer = useRef<number | undefined>(undefined);
  const syncProgressValue = useRef(0);
  // Set while a connect() is waiting to learn whether a Quad Cortex answers.
  const awaitingDetection = useRef(false);
  const connectionRef = useRef(connection);
  // One gateway for every workflow hook, routed to the real device or to the
  // demo device. Real-hardware-only paths (connect, live frames, recovery,
  // backups, the broker heartbeat) keep calling tauriTransport directly.
  const demoDeviceSession = useMemo(() => createDemoDeviceSession(tauriTransport), []);
  const activeGateway = demoDeviceSession.gateway;
  const [demoPending, setDemoPending] = useState(false);
  // The demo open that owns demoPending. A connect() can supersede an open
  // (connectionAttempts), and the superseded open must still clear the flag it
  // set, or the Demo device tab stays "Opening…" and disabled for good.
  const demoOpenToken = useRef(0);
  // Set by startDemoDevice when it leaves an already-ready Quad Cortex's
  // session open rather than closing it, so a person can switch back without
  // a full reconnect (resumeDeviceSession). deviceReady cannot carry this by
  // itself: it requires connection.source === "device", which is false for
  // the whole time the demo is the active routing target.
  const [deviceHeldInBackground, setDeviceHeldInBackground] = useState(false);
  const qcTransport = useMemo(() => createWindowsQcTransport(activeGateway, () => snapshotRef.current), [activeGateway]);
  connectionRef.current = connection;
  // beginLiveRecovery is memoized far above returnToDemoDevice's own
  // definition (and must stay stable - see its effect dependents), so it
  // cannot close over returnToDemoDevice directly without freezing on the
  // first render's copy, exactly the staleness connectOnDetection's ref
  // avoids for connect() below. Read through a ref for the same reason.
  const returnToDemoDeviceRef = useRef<(reason: string) => void>(() => undefined);
  const beginLiveRecovery = useCallback((cause: string) => {
    const current = connectionRef.current;
    if (current.source !== "device" || current.demo || current.phase !== "ready") return;
    nativeStateAvailable.current = false;
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "live-health-failed", result: "failure", detail: cause }]);
    // An unexpected hardware loss now returns to the demo device, exactly
    // like a reconnect attempted from one that fails or comes back
    // incomplete (returnToDemoDevice): the Grid stays fully editable instead
    // of sitting in an inert "needs-attention" state the switchable gateway
    // never actually routed to. Real hardware reconnection keeps being tried
    // in the background by the broker-phase listener below
    // (qcShouldConnectOnDetection allows it once the demo is the active,
    // ready session) - the same mechanism that already follows any other
    // failed connect, rather than the tight no-full-reconnect polling loop
    // this path used to lean on (see the live-sync effect's now-unreachable
    // `recovering` branch, kept only because dropping it made no code
    // simpler and nothing else still sets a "Live synchronization…" detail).
    returnToDemoDeviceRef.current(`the live connection was lost: ${cause}`);
  }, [setConnection]);
  const actionFailed = useCallback((error: unknown) => {
    const detail = error instanceof Error ? error.message : String(error);
    if (isQcConnectionFailure(detail)) beginLiveRecovery(detail);
    else setNotice(detail);
  }, [beginLiveRecovery]);
  const {
    status: relayStatus, pending: relayPending,
    start: startRelay, unpair: unpairRelay, setAccessMode: setRelayAccessMode
  } = usePublicRelayWorkflow({
    relay: publicRelay,
    enabled: Boolean(window.__TAURI_INTERNALS__),
    pollIntervalMs: 2000,
    autoStart: true
  });
  const workflows = useQcWorkflows({
    controller: qcController,
    transport: qcTransport,
    gateway: activeGateway,
    editor,
    selectedBlockId,
    setSelectedBlockId,
    connected: corpusFixtureEnabled || sessionReady,
    demo: corpusFixtureEnabled || connection.demo,
    source: connection.source,
    pending: commandPending,
    setPending: setCommandPending,
    prompts: appPrompts,
    panels: {
      // The generic advanced-routing dialog is retired on Windows (see the
      // routing-editor retirement note near openRoutingNodeEditor below):
      // nothing calls routingWorkflow.open() any more, so this callback has
      // nothing left to pop. Android still wires its own copy.
      // A block tap loads its live parameter editor directly (editor.load,
      // inside gridWorkflow.openBlock) with no host dialog to open alongside
      // it - it is not this callback's job to pop one. Nothing calls
      // openManagement (this hook's own name for it) here any more: it used
      // to be the sole way "Assign Expression Pedal" reached anything, and
      // that now opens the live expression-assignment screen instead.
      // Add Device is the unit's own device browser, opened over the live Grid.
      liveAddDevicePicker: true
      // Scene copy/swap opens CorOsSceneCommand from the Grid context menu.
      // Label/color follows the unit's own Scene mode -> Gig View -> tile Edit
      // path and is rendered as the current device screen. Neither uses this
      // hook's generic scenes panel, so openScenes/close have nothing to pop.
    },
    notice: setNotice,
    fail: actionFailed,
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
  const { undoEntry, redoEntry, record: recordUndo, clear: clearUndo } = deviceHistory;
  const expressionWorkflow = useExpressionAssignment({
    gateway: activeGateway,
    snapshot,
    connected: sessionReady,
    pending: commandPending,
    setPending: setCommandPending,
    reconcile: reconcileWorkflowSnapshot,
    recordHistory: recordUndo,
    notice: setNotice,
    fail: actionFailed
  });
  const deviceSettingsWorkflow = useDeviceSettingsWorkflow({
    gateway: activeGateway,
    connected: sessionReady,
    pending: commandPending,
    setPending: setCommandPending,
    notice: setNotice,
    fail: actionFailed
  });
  const tempoWorkflow = useTempoWorkflow({
    gateway: activeGateway,
    connected: sessionReady,
    pending: commandPending,
    setPending: setCommandPending,
    notice: setNotice,
    fail: actionFailed
  });
  const tunerWorkflow = useTunerWorkflow({ gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setCommandPending, notice: setNotice, fail: actionFailed });
  const looperWorkflow = useLooperXWorkflow({ gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setCommandPending, notice: setNotice, fail: actionFailed });
  const midiOutWorkflow = usePresetMidiOutWorkflow({ gateway: activeGateway, snapshot, demo: connection.demo || connection.source === "virtual", onSaved: reconcileWorkflowSnapshot });
  const modesWorkflow = useModesConfigurationWorkflow({ gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setCommandPending, notice: setNotice, fail: actionFailed });
  const ioEqWorkflow = useIoEqWorkflows({ gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setCommandPending, notice: setNotice, fail: actionFailed, snapshot });
  const followedScreenKind = useRef<QcScreenObservation["kind"] | undefined>(undefined);
  useEffect(() => {
    if (!followQcScreen || !deviceReady) followedScreenKind.current = undefined;
  }, [deviceReady, followQcScreen]);
  useAdaptiveQcScreenPolling({
    gateway: activeGateway,
    enabled: followQcScreen && !connection.demo && !corpusFixtureEnabled,
    connected: deviceReady,
    onObservation: (observation) => {
      if (followedScreenKind.current === observation.kind) return;
      followedScreenKind.current = observation.kind;
      const closeLocalOverlays = () => {
        gridWorkflow.close();
        presetWorkflow.closeDirectory();
        deviceSettingsWorkflow.close();
        tempoWorkflow.close();
        tunerWorkflow.close();
        setDeviceScreenView(null);
      };
      if (observation.kind === "parameter-editor" || observation.kind === "unknown") return;
      closeLocalOverlays();
      if (observation.kind === "tuner") { setDeviceScreenView("tuner"); void tunerWorkflow.openTuner(); }
      else if (observation.kind === "tempo") void tempoWorkflow.openTempo();
      else if (observation.kind === "gig") setDeviceScreenView("gig");
      else if (observation.kind === "settings") void deviceSettingsWorkflow.openSettings();
      else if (observation.kind === "io") { setDeviceScreenView("io-overview"); void ioEqWorkflow.refresh(); }
      else if (observation.kind === "directory") void presetWorkflow.openDirectory();
      else if (observation.kind === "cpu-monitor") setDeviceScreenView("cpu-monitor");
      else if (observation.kind === "looper") setDeviceScreenView("looper-editor");
    }
  });
  const continuousControls = useContinuousControlWorkflow({
    controller: qcController,
    gateway: activeGateway,
    connected: sessionReady,
    demo: corpusFixtureEnabled || virtualReady,
    reconcile: reconcileWorkflowSnapshot,
    recordHistory: recordUndo,
    notice: setNotice,
    fail: actionFailed
  });
  const presetList = presetWorkflow.presetList;
  const presetListLoading = presetWorkflow.directoryLoading;
  const presetFoldersLoading = presetWorkflow.directoryLoading;
  const presetDirectoryOpen = presetWorkflow.directoryOpen;
  const presetClipboard = presetWorkflow.clipboard;
  const savePresetName = presetWorkflow.saveName;
  const savePresetScreenOpen = presetWorkflow.saveOpen;
  const setPresetDirectoryOpen = (open: boolean) => open ? void presetWorkflow.openDirectory() : presetWorkflow.closeDirectory();
  const setSavePresetName = presetWorkflow.setSaveName;
  const setSavePresetScreenOpen = (open: boolean) => open ? presetWorkflow.openSave() : presetWorkflow.closeSave();
  const loadPresetDirectory = presetWorkflow.loadDirectory;
  const openPresetBrowser = presetWorkflow.openDirectory;
  const recallPreset = presetWorkflow.recall;
  const reloadPreset = presetWorkflow.reload;
  const copyCurrentPreset = presetWorkflow.copy;
  const pasteCurrentPreset = presetWorkflow.paste;
  const renameCurrentPreset = presetWorkflow.rename;
  const openDeviceSave = presetWorkflow.openSave;
  const savePresetToDevice = presetWorkflow.save;
  const setRoutePicker = (value: undefined) => { if (value === undefined) routingWorkflow.closePicker(); };
  const openRoutePicker = routingWorkflow.openPicker;
  // Move-to-an-empty-cell and its select-based picker were GridManagementPanel
  // only. The device's own block-context menu has no Move row at all - drag
  // on the live Grid is the one move mechanism, and gridWorkflow.moveBlockTo
  // already serves that directly - so neither the panel nor this state has a
  // reason to exist on Windows now that the panel is gone.
  const footswitchAssignmentPending = gridWorkflow.footswitchAssignmentPending;
  const setFootswitchAssignmentPending = gridWorkflow.setFootswitchAssignmentPending;
  const blockClipboard = gridWorkflow.clipboard;
  const openBlockEditor: typeof gridWorkflow.openBlock = async (block) => {
    setParameterMenuInitiallyOpen(false);
    // CorOS exposes Looper X through its Grid block. Tapping that block opens
    // the dedicated full-screen transport; it is not a second generic device
    // editor or a global Performance-menu screen.
    if (!connection.demo && connection.source !== "virtual" && /\blooper\b/i.test(`${block.category ?? ""} ${block.name}`)) {
      await looperWorkflow.run("open");
      setDeviceScreenView("looper-editor");
      return;
    }
    return gridWorkflow.openBlock(block);
  };
  const openRoutingNodeEditor = gridWorkflow.openRoutingNode;
  const applyFootswitchAssignment = gridWorkflow.assignFootswitch;
  const removeSelectedBlock = gridWorkflow.remove;
  const openAddBlock = gridWorkflow.openAdd;
  const addDevicePickerOpen = Boolean(gridWorkflow.addPickerProps);
  const closeAddDevicePicker = gridWorkflow.closeAddPicker;
  const copySelectedBlockSettings = gridWorkflow.copy;
  const pasteSelectedBlockSettings = gridWorkflow.paste;
  const draftParameterValue = parameterWorkflow.draft;
  const applyParameterBatch = parameterWorkflow.commitBatch;
  const parameterEditorBindings = qcParameterEditorBindings({
    titlePresetMark: corpusFixtureEnabled && editor.details?.name === "Simple Gate",
    snapshot,
    selectedBlockId,
    editor,
    grid: gridWorkflow,
    parameter: parameterWorkflow,
    performance: performanceWorkflow,
    connected: sessionReady,
    pending: commandPending,
    initialMenuOpen: parameterMenuInitiallyOpen,
    notice: setNotice,
    openExpression: () => { if (blockDetails) expressionWorkflow.openAssignment(blockDetails); }
  });
  const laneControlEditor = qcLaneControlPanelBindings({
    snapshot,
    selectedBlockId,
    editor,
    grid: gridWorkflow,
    performance: performanceWorkflow,
    pending: commandPending,
    notice: setNotice,
    openExpression: (control) => { if (blockDetails) expressionWorkflow.openAssignment(blockDetails, { kind: "lane", control }); }
  });
  const expressionAssignment = blockDetails ? expressionWorkflow.assignmentFor(blockDetails) : undefined;
  // A replacement broker restarts its frame numbering, and frames are deltas:
  // whatever the device did while the two producers swapped is in no frame at
  // all. Pull the whole state once so the UI rejoins the device where it is.
  const resyncAfterStreamRestart = useCallback((reason: "initial" | "restarted" | "gap") => {
    // Windows enables the pushed stream only after connect() has completed an
    // authoritative currentSnapshot read. Its first pushed frame therefore
    // adopts that already-synchronized stream; reading the same snapshot again
    // made every connection look like a stream failure immediately after the
    // successful preset sync.
    if (reason === "initial") return;
    pendingNativeStateRecovery.current = {
      revision: nativeStateRecoveryRevision.current,
      streamId: nativeStateStream.current,
      reason
    };
    reportedNativeStateRecoveryFailure.current = undefined;
    resetCommands();
    if (qcStreamRestartClearsBulkOperation(reason)) {
      setDeviceBulkBlocking(false);
    }
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: reason === "restarted" ? "state-stream-restarted" : "state-stream-interrupted", result: "warning", detail: reason === "restarted" ? "The device state producer changed; restoring the current preset state." : "Some live updates were missed; restoring the current preset state." }]);
    // useNativeStateRecovery owns the checked snapshot. Its revision poll runs
    // after this frame has been reduced and only while deviceReady permits
    // reads, so it cannot race the replacement frame or a suspend/bulk event.
  }, []);

  useNativeStateRecovery({
    enabled: deviceReady && !connection.demo,
    controller: qcController,
    sequence: nativeStateSequence,
    streamId: nativeStateStream,
    recoveryRevision: nativeStateRecoveryRevision,
    gridRepair,
    readEvents: (after, limit) => tauriTransport.currentStateEvents(after, limit),
    readSnapshot: () => tauriTransport.currentSnapshot(),
    readGridRepair: () => tauriTransport.currentPreset(),
    onRecoveryResult: (result) => {
      const pending = pendingNativeStateRecovery.current;
      if (!pending || result.revision !== pending.revision || result.streamId !== pending.streamId) return;
      if (result.status === "recovered") {
        pendingNativeStateRecovery.current = undefined;
        reportedNativeStateRecoveryFailure.current = undefined;
        setConnection((state) => ({ ...state, lastSync: new Date().toISOString() }));
        setConnectionEvents((events) => [...events, {
          at: new Date().toISOString(),
          event: "state-stream-resynchronized",
          result: "success",
          detail: `${result.snapshot.presetLocation} · ${result.snapshot.presetName} is up to date.`
        }]);
        return;
      }
      if (reportedNativeStateRecoveryFailure.current === result.revision) return;
      reportedNativeStateRecoveryFailure.current = result.revision;
      const detail = result.error instanceof Error ? result.error.message : String(result.error);
      setConnectionEvents((events) => [...events, {
        at: new Date().toISOString(),
        event: "state-stream-resync-failed",
        result: "failure",
        detail
      }]);
    }
  });

  useWindowsDeviceFrames({
    enabled: qcLiveSessionCanObserveState(connection),
    selectedDeviceId: selectedPhysicalRef,
    sequence: nativeStateSequence,
    streamId: nativeStateStream,
    recoveryRevision: nativeStateRecoveryRevision,
    available: nativeStateAvailable,
    consume: consumeLiveState,
    setSnapshot,
    onStreamRestart: resyncAfterStreamRestart
  });

  const stopSyncProgressAnimation = () => {
    if (syncProgressTimer.current !== undefined) window.clearInterval(syncProgressTimer.current);
    syncProgressTimer.current = undefined;
  };
  const showSyncProgress = (value: number | null) => {
    if (value === null) {
      syncProgressValue.current = 0;
      setSyncProgress(null);
      return;
    }
    const next = Math.max(syncProgressValue.current, Math.min(100, Math.round(value)));
    syncProgressValue.current = next;
    setSyncProgress(next);
  };
  const animateSyncProgress = (target: number, durationMs: number) => {
    stopSyncProgressAnimation();
    const start = syncProgressValue.current;
    const startedAt = performance.now();
    const tick = () => {
      const elapsed = performance.now() - startedAt;
      const ratio = Math.min(1, elapsed / durationMs);
      showSyncProgress(start + (target - start) * ratio);
      if (ratio === 1) stopSyncProgressAnimation();
    };
    tick();
    syncProgressTimer.current = window.setInterval(tick, 80);
  };
  // One curve, started only once a device is known to be there, and never
  // restarted: the broker phase and the reconnect result can both report the
  // same detection, and the second must not rewind the first.
  const beginSyncProgress = () => {
    if (syncProgressTimer.current !== undefined || syncProgressValue.current > 0) return;
    const startedAt = performance.now();
    const tick = () => showSyncProgress(syncProgressAt(performance.now() - startedAt));
    tick();
    syncProgressTimer.current = window.setInterval(tick, 80);
  };
  const finishSyncProgress = async (attempt: { current(): boolean }) => {
    const duration = syncProgressFinishMs(syncProgressValue.current);
    animateSyncProgress(100, duration);
    await new Promise((resolve) => window.setTimeout(resolve, duration + 180));
    if (attempt.current()) stopSyncProgressAnimation();
  };

  const formFactor = useMemo(() => formFactors.find((item) => item.id === formFactorId) ?? formFactors[0], [formFactorId]);
  const skin = useMemo(() => skins.find((item) => item.id === formFactor.defaultSkinId) ?? skins[0], [formFactor]);
  const presetLabel = `${snapshot.presetLocation} · ${snapshot.presetName}`;
  const visibleNotice = qcVisibleStatusNotice(notice, deviceReady, presetLabel);
  // Not a property of any pairing, so it is no longer drawn over the provider
  // rows: a second copy of QC Remote serving the AI apps, local access switched
  // off, and a listener that never started are all facts about this app.
  // Settings states them, beside the other app-wide AI control.
  const localAccessCondition = localAccessNotice(localAccessStatus);
  // The QC stays fully usable while a catalog pull streams, so the pill keeps
  // saying LIVE and only annotates that something is still arriving. The
  // waiting signal is gated on the device still being live: presetFoldersPending
  // is cleared only by a successful load, so a QC unplugged mid-pull would
  // otherwise leave "waiting for data" on screen forever, pointed at nothing.
  const awaitingDeviceData = deviceReady && presetWorkflow.catalogPending;
  const usbActionRequired = Boolean(runtime?.usbDiagnostics?.writesQuarantined);
  const liveIndicator = qcLiveIndicator(connection, deviceReady, syncProgress, awaitingDeviceData, { actionRequired: usbActionRequired });
  const appMenus = useMemo<AppMenu[]>(() => {
    const ready = sessionReady && !commandPending;
    const hasSelectedBlock = snapshot.blocks.some((block) => block.id === selectedBlockId && block.column >= 0 && block.modelId !== undefined);
    const clipboardCompatible = Boolean(blockClipboard && snapshot.blocks.some((block) => block.id === selectedBlockId && block.modelId === blockClipboard.modelId));
    const canCopyPreset = ready && !snapshot.dirty && snapshot.presetName !== "Unsaved";
    const canPastePreset = Boolean(ready && presetClipboard && !snapshot.dirty && (
      presetClipboard.setlistKey !== snapshot.setlistKey || presetClipboard.presetPosition !== snapshot.presetPosition
    ));
    return [
      { name: "Preset", items: [
        { id: "open-workspace", label: "Open Preset…" },
        { id: "save-workspace", label: "Save Preset", shortcut: "Ctrl+S" },
        { id: "save-workspace-as", label: "Save Preset As…" },
        { id: "open-preset-directory", label: "Preset Library…", disabled: !ready },
        { id: "save-preset-to-device", label: "Save to QC…", shortcut: "Ctrl+Shift+S", disabled: !ready },
        { id: "rename-current-preset", label: "Rename Current Preset…", disabled: !ready },
        divider(),
        { id: "add-block", label: "Add Device…", disabled: !ready },
        { id: "edit-routing", label: "Routing…", disabled: !ready },
        { id: "discard-changes", label: "Discard Changes…", disabled: !ready || !snapshot.dirty },
        divider(),
        { id: "export-preset-library", label: "Export Preset Library", disabled: true },
        { id: "device-backup", label: "Device Backup", disabled: !deviceReady || commandPending },
        divider(),
        { id: "exit", label: "Exit" }
      ] },
      { name: "Edit", items: [
        { id: "undo", label: undoEntry ? `Undo ${undoEntry.label}` : "Undo", shortcut: "Ctrl+Z", disabled: (!undoEntry && !deviceUndoAvailable) || !ready },
        { id: "redo", label: redoEntry ? `Redo ${redoEntry.label}` : "Redo", shortcut: "Ctrl+Y", disabled: (!redoEntry && !deviceRedoAvailable) || !ready },
        divider(),
        { id: "copy-preset", label: "Copy Preset", disabled: !canCopyPreset },
        { id: "paste-preset", label: "Paste Preset", disabled: !canPastePreset },
        { id: "copy-preset-text", label: "Copy Preset as Text", disabled: !ready || !snapshot.presetIdentity },
        { id: "paste-preset-text", label: "Paste Preset from Text", disabled: !ready || !snapshot.presetIdentity },
        { id: "copy-setlist", label: "Copy Setlist", disabled: !ready || !snapshot.setlistKey },
        { id: "paste-setlist", label: "Paste Setlist", disabled: !ready || !setlistClipboard },
        divider(),
        { id: "copy-block-settings", label: "Copy Device", shortcut: "Ctrl+C", disabled: !hasSelectedBlock },
        { id: "paste-block-settings", label: "Paste Device", shortcut: "Ctrl+V", disabled: !ready || !clipboardCompatible },
        divider(),
        { id: "settings", label: "Settings…" }
      ] },
      { name: "View", items: [
        { id: "view-fit", label: "Fit to Window", checked: surfaceView === "fit" },
        { id: "view-actual", label: "Actual Size", checked: surfaceView === "actual" },
        { id: "toggle-fullscreen", label: "Full Screen", checked: fullScreen }
      ] },
      { name: "Performance", items: [
        { id: "previous-preset", label: "Previous Preset", disabled: !ready || snapshot.dirty },
        { id: "next-preset", label: "Next Preset", disabled: !ready || snapshot.dirty },
        divider(),
        { id: "set-tempo", label: "Tempo…", disabled: !ready },
        ...(snapshot.modeSlots ?? []).map((entry): MenuItem => ({ id: `select-mode-${entry.slot}`, label: entry.label, checked: snapshot.mode === entry.mode, disabled: !ready })),
        divider(),
        { id: "open-io-settings", label: "I/O Settings", disabled: !ready },
        { id: "open-tuner", label: "Tuner", disabled: !ready },
        { id: "open-gig-view", label: "Gig View", disabled: !ready }
      ] },
      { name: "Help", items: [
        { id: "user-guide", label: "User Guide", shortcut: "F1" },
        { id: "export-diagnostics", label: "Export Diagnostics…" },
        divider(),
        { id: "report-bug", label: "Report an Issue…" },
        { id: "send-feedback", label: "Send Feedback…" },
        divider(),
        { id: "about", label: `About ${QC_BRAND.appName}…` }
      ] }
    ];
  }, [blockClipboard, commandPending, deviceReady, deviceRedoAvailable, deviceUndoAvailable, fullScreen, presetClipboard, redoEntry, selectedBlockId, sessionReady, setlistClipboard, snapshot.blocks, snapshot.dirty, snapshot.mode, snapshot.modeSlots, snapshot.presetIdentity, snapshot.presetName, snapshot.presetPosition, snapshot.setlistKey, surfaceView, undoEntry]);

  useEffect(() => {
    const sync = () => setFullScreen(Boolean(document.fullscreenElement));
    document.addEventListener("fullscreenchange", sync);
    return () => document.removeEventListener("fullscreenchange", sync);
  }, []);

  useEffect(() => {
    const attempt = connectionAttempts.capture();
    void tauriTransport.runtimeStatus().then((status) => {
      if (!attempt.current()) return;
      setRuntime(status);
      setConnectionEvents((current) => [...current, { at: new Date().toISOString(), event: "runtime-ready", result: status.gatewayAvailable ? "success" : "warning", detail: status.message || (status.gatewayAvailable ? "Device gateway is available." : "Device gateway is unavailable.") }]);
    }).catch((error: Error) => { if (attempt.current()) setNotice(error.message); });
    return () => {
      if (syncProgressTimer.current !== undefined) window.clearInterval(syncProgressTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!window.__TAURI_INTERNALS__ || corpusFixtureEnabled) return;
    let cancelled = false;
    let timer: number | undefined;
    const schedule = () => {
      // This is a local broker heartbeat, not a request to the QC. Keeping it
      // fast prevents stale green UI without adding USB polling traffic.
      if (!cancelled) timer = window.setTimeout(() => void monitorDeviceHealth(), 250);
    };
    const monitorDeviceHealth = async () => {
      const attempt = connectionAttempts.capture();
      try {
        const status = await tauriTransport.runtimeStatus();
        if (cancelled || !attempt.current()) return;
        setRuntime(status);
        if (connectionRef.current.phase === "ready" && !connectionRef.current.demo && connectionRef.current.source === "device") {
          if (!runtimeHasSynchronizedQc(status)) {
            beginLiveRecovery(status.usbDiagnostics?.detail || status.message || "The Quad Cortex is not synchronized");
          }
        }
        // Writes can be quarantined (broker-side, after a write whose device
        // outcome came back unconfirmed) without the device becoming
        // unsynchronized, so this is a separate check from the one above -
        // and, unlike beginLiveRecovery, it must not fall back to the demo
        // device: reads still work, and only a session reset (not a
        // reconnect) clears it. Without this the app polled the field every
        // 250ms and never surfaced it anywhere.
        // The broker's session-scoped suspension record is authoritative: it
        // clears with the session, so a lost resume frame cannot leave every
        // control disabled after a reconnect. An older broker omits it.
        if (status.usbDiagnostics?.suspension !== undefined) {
          setDeviceSuspended(Boolean(status.usbDiagnostics.deviceSuspended));
        }
        observeWriteHealth(Boolean(status.usbDiagnostics?.writesQuarantined), status.usbDiagnostics?.writeQuarantineReason);
      } catch (error) {
        if (cancelled || !attempt.current()) return;
        const detail = error instanceof Error ? error.message : String(error);
        setRuntime((current) => ({
          platform: current?.platform ?? "Windows desktop runtime",
          gatewayAvailable: false,
          message: detail
        }));
        beginLiveRecovery(detail);
      } finally {
        schedule();
      }
    };
    schedule();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [beginLiveRecovery, observeWriteHealth]);

  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    void setRelayAccessMode(controlAccessMode).catch(actionFailed);
  }, [actionFailed, controlAccessMode, setRelayAccessMode]);

  const beginBrowserRelayPairing = useCallback(async () => {
    if (relayPairingStartInFlight.current) return;
    relayPairingStartInFlight.current = true;
    setRelayPairingStarting(true);
    setRelayPairingError(undefined);
    try {
      const offer = await publicRelay.browserPairing!.begin();
      if (!offer.pairUrl) throw new Error("The desktop relay did not return a pairing URL.");
      const qrCode = await QRCode.toDataURL(offer.pairUrl, { width: 224, margin: 1, errorCorrectionLevel: "M" });
      setRelayBrowserPairing({ pairUrl: offer.pairUrl, expiresAt: offer.expiresAt, qrCode });
    } catch (error) {
      setRelayPairingError(error instanceof Error ? error.message : "QC Remote could not create a pairing code.");
      actionFailed(error);
    } finally {
      relayPairingStartInFlight.current = false;
      setRelayPairingStarting(false);
    }
  }, [actionFailed]);

  const refreshRelayConnectors = useCallback(async () => {
    if (!publicRelay.connectors) return;
    try {
      setRelayConnectors(await publicRelay.connectors());
      setRelayConnectorsError(undefined);
    } catch (error) {
      setRelayConnectors([]);
      setRelayConnectorsError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  // A pairing code lasts ten minutes; a panel left open outlives that easily.
  // Reporting the expiry made the person press Refresh to get back exactly
  // where they were, so the code is replaced in place instead and the panel
  // never shows a dead QR. Closing REMOTE still withdraws it, so this only
  // ever renews something already on screen.
  useEffect(() => {
    if (!relayBrowserPairing) return;
    const delay = Math.max(0, relayBrowserPairing.expiresAt - Date.now());
    const timer = window.setTimeout(() => {
      if (!relayPanelOpen.current) return;
      void beginBrowserRelayPairing();
    }, delay);
    return () => window.clearTimeout(timer);
  }, [beginBrowserRelayPairing, relayBrowserPairing]);

  useEffect(() => {
    if (!relayBrowserPairing) return;
    let active = true;
    const check = async () => {
      try {
        const result = await publicRelay.browserPairing!.poll();
        if (!active) return;
        if (result.state === "paired") {
          setRelayBrowserPairing(undefined);
          setNotice("Remote access is connected.");
          void refreshRelayConnectors();
          // Pairing used to end the panel's pairing surface outright, so a
          // second device meant unpairing the first. The code comes back while
          // the panel is open, and the device just added is the top row of the
          // list under it.
          if (relayPanelOpen.current) void beginBrowserRelayPairing();
        }
      } catch (error) {
        if (!active) return;
        setRelayBrowserPairing(undefined);
        actionFailed(error);
      }
    };
    void check();
    const timer = window.setInterval(() => void check(), 2_000);
    return () => { active = false; window.clearInterval(timer); };
  }, [actionFailed, beginBrowserRelayPairing, refreshRelayConnectors, relayBrowserPairing]);

  const shareRelayPairing = useCallback(async () => {
    const pairUrl = relayBrowserPairing?.pairUrl;
    if (!pairUrl) return;
    try {
      if (typeof navigator.share === "function") await navigator.share({ title: "Pair device", url: pairUrl });
      else await navigator.clipboard.writeText(pairUrl);
      setNotice("Pair link is ready to share.");
    } catch (error) {
      // A user declining the platform share sheet is not an application error.
      if (error instanceof DOMException && error.name === "AbortError") return;
      actionFailed(error);
    }
  }, [actionFailed, relayBrowserPairing]);

  const reconnectPublicRelay = async () => {
    try {
      await startRelay();
      setNotice("Public MCP relay reconnect started.");
    } catch (error) {
      actionFailed(error);
    }
  };


  const unpairPublicRelay = async () => {
    try {
      const status = await unpairRelay();
      setRelayBrowserPairing(undefined);
      setRelayPairingError(undefined);
      setRelayConnectors([]);
      setNotice(status?.detail ?? "Remote access was disconnected and unpaired.");
      void refreshRelayConnectors();
      // Unpairing this computer leaves the panel with nothing paired, which is
      // the state a live code belongs in most.
      if (relayPanelOpen.current) void beginBrowserRelayPairing();
    } catch (error) {
      actionFailed(error);
    }
  };

  const openRemoteAccountDeletionPage = async () => {
    try {
      await externalLinks.open(relayProfile.accountDeletionInformationUrl);
    } catch {
      setNotice("Could not open the account deletion page. Try again when a browser is available.");
    }
  };

  const unpairRelayAccount = async () => {
    if (!await appPrompts.confirm("Unpair this Quad Cortex from Remote MCP Cloud Relay? Linked AI apps will no longer be able to reach it.")) return;
    await unpairPublicRelay();
  };

  const unlinkRelayConnector = async (grantId: string) => {
    const connector = relayConnectors.find((row) => row.grantId === grantId);
    if (!connector || !publicRelay.unlinkConnector || relayUnlinkingGrantId) return;
    if (!await appPrompts.confirm(`Unlink ${connector.name}? Its Remote MCP authorization will be revoked immediately.`)) return;
    setRelayUnlinkingGrantId(grantId);
    try {
      setRelayConnectors(await publicRelay.unlinkConnector(grantId));
      setRelayConnectorsError(undefined);
      setNotice(`${connector.name} unlinked from Remote MCP.`);
    } catch (error) {
      actionFailed(error);
      await refreshRelayConnectors();
    } finally {
      setRelayUnlinkingGrantId(undefined);
    }
  };

  /** Share the address an AI app needs to reach this account. Native sharing is
   * preferred on hosts that expose it; clipboard is the desktop fallback. */
  const shareRemoteConnectorAddress = async () => {
    const url = `${publicRelayEndpoint}/mcp`;
    try {
      if (navigator.share) {
        await navigator.share({ title: "Remote MCP Connector", text: "Connect an AI app to this Quad Cortex.", url });
        setNotice("Remote MCP Connector shared.");
      } else {
        await navigator.clipboard.writeText(url);
        setNotice("Remote MCP Connector copied. Share it with the AI app you want to link.");
      }
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") return;
      actionFailed(error);
    }
  };

  /** Open the pairing page in the system browser.
   *
   * Signing in on this machine is a legitimate way to pair, and was previously
   * reachable only by sharing the link to yourself. The native side re-validates
   * the address against the configured relay origin, so this cannot become a
   * general "open any URL" for the web layer. */
  const openRelayPairUrl = async () => {
    const pairUrl = relayBrowserPairing?.pairUrl;
    if (!pairUrl) return;
    try {
      await publicRelay.browserPairing!.openPairUrl!(pairUrl);
    } catch (error) {
      actionFailed(error);
    }
  };

  const openRelayConnector = async () => {
    const address = `${publicRelayEndpoint}/mcp`;
    try {
      await publicRelay.browserPairing!.openPairUrl!(address);
    } catch (error) {
      actionFailed(error);
    }
  };

  /** REMOTE opened or closed.
   *
   * Opening creates the pairing code, paired or not: the code is the panel's
   * constant, and requiring a button press to reveal it was a step that told
   * nobody anything. Closing withdraws it, so a code is never live against a
   * screen nobody is looking at. */
  const setRemotePanelOpen = useCallback((open: boolean) => {
    if (relayPanelOpen.current === open) return;
    relayPanelOpen.current = open;
    if (open) {
      void refreshRelayConnectors();
      if (!relayBrowserPairing && !relayPairingStarting) void beginBrowserRelayPairing();
      return;
    }
    void publicRelay.browserPairing!.cancel!().catch(() => undefined);
    setRelayBrowserPairing(undefined);
  }, [beginBrowserRelayPairing, refreshRelayConnectors, relayBrowserPairing, relayPairingStarting]);

  const chooseScene = performanceWorkflow.selectScene;
  const toggleSelectedBypass = useCallback(() => {
    const block = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId);
    if (block) void performanceWorkflow.toggleBlockBypass(block);
  }, [performanceWorkflow, selectedBlockId, snapshot.blocks]);
  const pressFootswitch = performanceWorkflow.pressFootswitch;
  const showDeviceView = useCallback(async (view: "tuner" | "gig") => {
    if (await performanceWorkflow.showDeviceView(view, true)) {
      if (view === "tuner") tunerWorkflow.openTuner();
      setDeviceScreenView(view);
    }
  }, [performanceWorkflow, tunerWorkflow]);
  const showDeviceInformation = useCallback(async () => {
    if (!sessionReady) {
      setNotice("Connect a Quad Cortex or start the Demo device before opening Device Information.");
      return;
    }
    setDeviceScreenView("settings-info");
    try {
      const identity = await activeGateway.identity();
      setDeviceInformation({
        serial: identity.serial,
        deviceName: identity.customName || snapshotRef.current.deviceName,
        corOsVersion: identity.appFwVersion
      });
    } catch (error) {
      actionFailed(error);
    }
  }, [actionFailed, activeGateway, sessionReady, snapshotRef]);
  const showIoSettings = useCallback(async () => {
    if (connection.demo || connection.source === "virtual") {
      setDeviceScreenView("io-overview");
      setNotice("Demo: I/O Settings opened.");
      return;
    }
    if (!deviceReady) {
      setNotice("Connect the Quad Cortex before opening I/O Settings.");
      return;
    }
    try {
      await ioEqWorkflow.refresh();
      setDeviceScreenView("io-overview");
      setNotice("I/O Settings opened.");
    } catch (error) {
      actionFailed(error);
    }
  }, [actionFailed, activeGateway, connection.demo, connection.source, deviceReady, ioEqWorkflow]);
  const closeDeviceScreen = useCallback(async () => {
    if (deviceScreenView?.startsWith("io-")) {
      if (connection.demo || connection.source === "virtual") {
        setDeviceScreenView(null);
        setNotice("Demo: I/O Settings closed.");
        return;
      }
      setDeviceScreenView(null);
      return;
    }
    if (deviceScreenView === "looper-editor") { await looperWorkflow.run("close"); setDeviceScreenView(null); return; }
    const view = deviceScreenView?.startsWith("gig") ? "gig" : deviceScreenView?.startsWith("tuner") ? "tuner" : undefined;
    if (!view) { setDeviceScreenView(null); return; }
    if (await performanceWorkflow.showDeviceView(view, false)) setDeviceScreenView(null);
  }, [actionFailed, activeGateway, connection.demo, connection.source, deviceScreenView, looperWorkflow, performanceWorkflow]);
  const forwardDeviceScreenGesture = useCallback(async (x: number, y: number, toX: number, toY: number, kind: "tap" | "long-tap" | "swipe") => {
    if (connection.demo || connection.source === "virtual") return;
    if (!deviceReady) {
      setNotice("The Quad Cortex disconnected before the screen gesture could be sent.");
      return;
    }
    try {
      if (kind === "long-tap") await activeGateway.longTapScreen(x, y);
      else if (kind === "tap") await activeGateway.tapScreen(x, y);
      else await activeGateway.swipeScreen(x, y, toX, toY);
    } catch (error) {
      actionFailed(error);
    }
  }, [actionFailed, activeGateway, connection.demo, connection.source, deviceReady]);
  const navigateBank = performanceWorkflow.navigateBank;
  const navigatePreset = performanceWorkflow.movePreset;

  const adjustEditorParameter = useCallback((role: string, delta: number) => {
    return parameterWorkflow.adjustEncoder(role, delta, setNotice);
  }, [parameterWorkflow]);

  const queueTempo = continuousControls.queueTempo;
  const adjustTempo = continuousControls.adjustTempo;
  const adjustMasterVolume = continuousControls.adjustMasterVolume;
  const tapTempo = performanceWorkflow.tapTempo;
  const chooseModeSlot = performanceWorkflow.selectModeSlot;

  const moveBlockSelection = useCallback((key: string) => {
    const blocks = snapshot.blocks.filter((block) => block.column >= 0).sort((a, b) => a.row - b.row || a.column - b.column);
    if (!blocks.length) return;
    const current = blocks.find((block) => block.id === selectedBlockId) ?? blocks[0];
    const candidates = key === "ArrowLeft"
      ? blocks.filter((block) => block.row === current.row && block.column < current.column).sort((a, b) => b.column - a.column)
      : key === "ArrowRight"
        ? blocks.filter((block) => block.row === current.row && block.column > current.column).sort((a, b) => a.column - b.column)
        : key === "ArrowUp"
          ? blocks.filter((block) => block.row < current.row).sort((a, b) => b.row - a.row || Math.abs(a.column - current.column) - Math.abs(b.column - current.column))
          : blocks.filter((block) => block.row > current.row).sort((a, b) => a.row - b.row || Math.abs(a.column - current.column) - Math.abs(b.column - current.column));
    const next = candidates[0] ?? current;
    setSelectedBlockId(next.id);
    setNotice(`${next.name} selected.`);
  }, [selectedBlockId, snapshot.blocks]);

  const handleHardwareAction = useQcSurfaceActions({
    snapshot,
    selectedBlockId,
    blockDetails,
    grid: gridWorkflow,
    performance: performanceWorkflow,
    openBlock: (block) => { void openBlockEditor(block); },
    closeBlock: () => { void gridWorkflow.closeOnDevice(); },
    openRoutingNode: (row, node) => { void openRoutingNodeEditor(row, node); },
    openLaneControl: (row, control) => { void gridWorkflow.openLaneControl(row, control); },
    openSplitPlacement: (row) => routingWorkflow.openSplitPlacement(row),
    rotate: (role, delta) => {
      if (adjustEditorParameter(role, delta)) return;
      if (role === "tempo") adjustTempo(delta);
      else if (role === "master-volume") adjustMasterVolume(delta);
      else setNotice(connection.demo ? `Demo encoder: ${role} ${delta > 0 ? "+" : "−"}1.` : `${role} has no verified encoder action on the current screen.`);
    },
    editorUnhandled: (action) => {
      if (action.role === "bank:up") setNotice("BANK UP cycles models on the hardware. Model replacement stays disabled until the gateway can perform it atomically.");
      else setNotice(`${action.role} is mapped to the on-screen parameter above it; drag or rotate it to adjust the value.`);
    },
    unhandled: (action) => {
      if (action.kind !== "switch" || action.phase !== "release") return;
      if (action.role === "power") setNotice("Power and lock actions are intentionally available only on the physical Quad Cortex.");
      else setNotice(connection.demo ? `Demo switch: ${action.role}. Hardware was not changed.` : `${action.role} has no verified action on the current screen.`);
    }
  });

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      const textEntry = target?.matches("input, textarea, [contenteditable=true]");
      if (textEntry) return;
      if (target?.matches("button, select")) return;
      if (appMenuOpen || sceneWorkflow.command || sceneWorkflow.labelColorOpen || deviceSettingsWorkflow.open || presetDirectoryOpen || addDevicePickerOpen || tempoWorkflow.open) return;
      // The Grid destinations that fill the screen: their Grid and its
      // device shortcuts are not on screen. Gig View, Tuner and Looper keep
      // their single-key footswitch shortcuts.
      if (deviceScreenView === "modes" || deviceScreenView === "midi-out" || deviceScreenView === "cpu-monitor" || presetWorkflow.saveProps.open) return;
      if (blockDetails) {
        if (footswitchAssignmentPending && /^[1-8]$/.test(event.key)) {
          event.preventDefault();
          const index = Number(event.key) - 1;
          const assigned = snapshot.blocks.find((block) => block.id === selectedBlockId)?.footswitch;
          setFootswitchAssignmentPending(false);
          void applyFootswitchAssignment(assigned === index ? null : index);
          return;
        }
        if (event.key === "Escape") {
          event.preventDefault();
          editor.close();
          parameterWorkflow.cancel();
          setNotice("Returned to the Grid.");
        }
        return;
      }
      if (/^[1-8]$/.test(event.key)) event.ctrlKey ? void chooseScene(Number(event.key) - 1) : void pressFootswitch(Number(event.key) - 1);
      if (event.key === "[") void navigateBank(-1);
      if (event.key === "]") void navigateBank(1);
      if (event.key.toLowerCase() === "t") event.shiftKey ? void showDeviceView("tuner") : tapTempo();
      if (event.key.toLowerCase() === "b" && selectedBlockId) void toggleSelectedBypass();
      if (event.key === "Delete" && selectedBlockId) void removeSelectedBlock();
      if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) { event.preventDefault(); moveBlockSelection(event.key); }
      if (event.key === "Enter") {
        const block = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId);
        if (block) void openBlockEditor(block);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [addDevicePickerOpen, appMenuOpen, applyFootswitchAssignment, blockDetails, chooseScene, deviceScreenView, deviceSettingsWorkflow, footswitchAssignmentPending, moveBlockSelection, navigateBank, openBlockEditor, presetDirectoryOpen, presetWorkflow.saveProps.open, pressFootswitch, removeSelectedBlock, sceneWorkflow, selectedBlockId, showDeviceView, snapshot.blocks, tapTempo, tempoWorkflow, toggleSelectedBypass]);

  /**
   * Leave the demo device, if it is active, for the real one. Only the route
   * changes: the demo store stays on disk exactly as last saved, and the next
   * "Work with a demo device" reopens it from there.
   */
  const suspendDemoDevice = (): boolean => {
    if (demoDeviceSession.source !== "virtual") return false;
    demoDeviceSession.useDevice();
    // Undo entries replay against the transport that recorded them; none may
    // cross from the demo device to the hardware or back.
    clearUndo();
    presetWorkflow.resetCache();
    setSelectedBlockId("");
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-suspended", result: "info", detail: "The demo device was set aside; its presets stay saved on this computer." }]);
    return true;
  };

  /**
   * A reconnect attempted from a working demo session that does not reach a
   * ready Quad Cortex goes back to the demo, rather than leaving the user with
   * neither device. This is the one automatic return: a real session that was
   * live and then dropped still never falls back to the demo by itself.
   */
  const returnToDemoDevice = (reason: string) => {
    const returning = { phase: "discovering" as const, detail: "The Quad Cortex was not reached; returning to the demo device…", demo: true, source: "device" as const };
    connectionRef.current = returning;
    setConnection(returning);
    setNotice(returning.detail);
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-returning", result: "info", detail: reason }]);
    void startDemoDevice(reason);
  };
  returnToDemoDeviceRef.current = returnToDemoDevice;

  // `userAction` is set only for the user's own Connect, USB, device or menu click:
  // it supersedes a recorded Quit first. Mount and device detection leave it unset
  // and keep respecting the Quit.
  const connect = async (mode: "reconnect" | "reset" = "reconnect", physicalId?: string, userAction = false) => {
    if (physicalId && (commandPending || demoPending || awaitingDetection.current || connectionRef.current.phase === "syncing")) return;
    const attempt = connectionAttempts.begin();
    // Tried from a working demo session: the demo's preset must not sit on
    // screen as if it were still active while the hardware is being reached,
    // so the Grid shows a neutral placeholder until one of the two replaces it.
    const fromDemo = suspendDemoDevice();
    if (fromDemo) setSnapshot(CONNECTING_PLACEHOLDER_SNAPSHOT);
    // A full handshake here always ends with source "device" one way or the
    // other (ready, or a fallback back to the demo); either way a device
    // held in the background is no longer a distinct state from this attempt on.
    setDeviceHeldInBackground(false);
    resetCommands();
    setCommandPending(false);
    nativeStateSequence.current = 0;
    nativeStateStream.current = undefined;
    gridRepair.current = createGridRepairCoordinator();
    nativeStateAvailable.current = false;
    setConnectionEvents((current) => [...current, { at: new Date().toISOString(), event: mode === "reset" ? "reset-started" : "connection-started", result: "pending", detail: mode === "reset" ? "Restarting the private gateway session and USB handshake." : "Discovering the gateway and opening the Quad Cortex session." }]);
    stopSyncProgressAnimation();
    showSyncProgress(null);
    // Gray, with no bar, until HID enumeration finds a Quad Cortex. The bar
    // starts when the broker reports its open/handshake phase (see the device
    // phase listener), or when a legacy broker's reconnect comes back ready.
    awaitingDetection.current = true;
    const discovering = { phase: "discovering" as const, detail: "Looking for a Quad Cortex…", demo: true, source: "device" as const };
    connectionRef.current = discovering;
    setConnection(discovering);
    setNotice(discovering.detail);
    try {
      if (userAction) await brokerDevice.acknowledgeUserConnect();
      if (physicalId && physicalId !== selectedPhysicalRef.current) {
        const previousId = selectedPhysicalRef.current;
        selectedPhysicalRef.current = physicalId;
        setSelectedPhysicalId(physicalId);
        clearUndo();
        presetWorkflow.resetCache();
        setSnapshot(CONNECTING_PLACEHOLDER_SNAPSHOT);
        try { await physicalDevices.select(physicalId); }
        catch (error) {
          selectedPhysicalRef.current = previousId;
          setSelectedPhysicalId(previousId);
          throw error;
        }
        if (!attempt.current()) return;
      }
      const next = mode === "reset" ? await tauriTransport.resetSession() : await tauriTransport.reconnect();
      if (!attempt.current()) return;
      awaitingDetection.current = false;
      if (next.phase === "ready") {
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "gateway-connected", result: "success", detail: next.detail }]);
        beginSyncProgress();
        const syncing = { ...next, phase: "syncing" as const, detail: "Quad Cortex connected; reading the active preset…", lastSync: undefined };
        connectionRef.current = syncing;
        setConnection(syncing);
        setNotice(syncing.detail);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "preset-sync-started", result: "pending", detail: "Reading the active preset, routes, scenes, assignments, and parameter state." }]);
        liveSyncFailures.current = 0;
        presetWorkflow.resetCache();
        const current = await tauriTransport.currentSnapshot();
        if (!attempt.current()) return;
        let synchronizedVolume = current.masterVolume;
        for (let volumeAttempt = 0; volumeAttempt < 20; volumeAttempt += 1) {
          try {
            synchronizedVolume = (await tauriTransport.currentMasterVolume()).value;
            if (!attempt.current()) return;
            break;
          } catch {
            if (!attempt.current()) return;
            if (volumeAttempt === 19) break;
            await new Promise((resolve) => window.setTimeout(resolve, 50));
            if (!attempt.current()) return;
          }
        }
        const synchronized = { ...current, masterVolume: synchronizedVolume };
        const verifiedRuntime = await tauriTransport.runtimeStatus();
        if (!attempt.current()) return;
        if (!runtimeHasSynchronizedQc(verifiedRuntime)) {
          throw new Error(verifiedRuntime.usbDiagnostics?.detail || "The Quad Cortex preset is not synchronized");
        }
        setRuntime(verifiedRuntime);
        setSnapshot(synchronized);
        setSelectedBlockId("");
        await finishSyncProgress(attempt);
        if (!attempt.current()) return;
        setNotice(`${next.detail}. Active preset synchronized; preset folders and the model catalog will load only when opened.`);
        const ready = { ...next, phase: "ready" as const, demo: false, lastSync: new Date().toISOString() };
        connectionRef.current = ready;
        setConnection(ready);
        showSyncProgress(null);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "preset-sync-complete", result: "success", detail: `${synchronized.presetLocation} · ${synchronized.presetName} synchronized successfully.` }]);
      } else {
        stopSyncProgressAnimation();
        showSyncProgress(null);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "connection-incomplete", result: "warning", detail: next.detail }]);
        if (fromDemo) { returnToDemoDevice(next.detail); return; }
        connectionRef.current = next;
        setConnection(next);
        setNotice(next.detail);
      }
    } catch (error) {
      if (!attempt.current()) return;
      awaitingDetection.current = false;
      const detail = error instanceof Error ? error.message : String(error);
      stopSyncProgressAnimation();
      showSyncProgress(null);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "connection-failed", result: "failure", detail }]);
      if (fromDemo) { returnToDemoDevice(detail); return; }
      const failed = { phase: "needs-attention" as const, detail, demo: true, source: "device" as const };
      connectionRef.current = failed;
      setConnection(failed);
      setNotice(detail);
    }
  };

  const disconnectDevice = async () => {
    if (commandPending) {
      setNotice("Wait for the current device command to finish before disconnecting.");
      return;
    }
    const attempt = connectionAttempts.begin();
    resetCommands();
    awaitingDetection.current = false;
    stopSyncProgressAnimation();
    showSyncProgress(null);
    setCommandPending(true);
    const disconnecting = { phase: "disconnected" as const, detail: "Closing the Quad Cortex session…", demo: true, source: "device" as const };
    // Revoke Ready before closing USB. This also prevents the liveness monitor
    // from misclassifying an intentional close as an unexpected device loss.
    connectionRef.current = disconnecting;
    setConnection(disconnecting);
    setNotice(disconnecting.detail);
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnect-started", result: "pending", detail: "Closing the live device and gateway session." }]);
    try {
      const next = await brokerDevice.disconnect();
      if (!attempt.current()) return;
      nativeStateSequence.current = 0;
      nativeStateStream.current = undefined;
      gridRepair.current = createGridRepairCoordinator();
      nativeStateAvailable.current = false;
      liveSyncFailures.current = 0;
      resetCommands();
      connectionRef.current = next;
      setConnection(next);
      setNotice(next.detail);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnected", result: "success", detail: next.detail }]);
    } catch (error) {
      if (!attempt.current()) return;
      const detail = error instanceof Error ? error.message : String(error);
      const failed = { phase: "needs-attention" as const, detail, demo: true, source: "device" as const };
      connectionRef.current = failed;
      setConnection(failed);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnect-failed", result: "failure", detail }]);
      setNotice(detail);
    } finally {
      if (attempt.current()) setCommandPending(false);
    }
  };

  /**
   * Open the demo device against its last saved state and route every
   * command to it.
   *
   * This used to be offered only by an explicit press while no real Quad
   * Cortex was live. It remains the launch default and the fallback after an
   * unexpected hardware loss (returnToDemoDevice, reached from
   * beginLiveRecovery). The USB panel's explicit Disconnect instead leaves the
   * broker and app offline. A person can still step away from an already-
   * connected, ready Quad Cortex without disconnecting it by selecting Demo -
   * see `keepDeviceInBackground`
   * below, and resumeDeviceSession for the way back that does not repeat the
   * handshake.
   */
  const startDemoDevice = async (returningFrom?: string) => {
    // A connect() attempt still awaiting its answer owns the next step
    // itself - awaitingDetection.current stays true for the whole discovering
    // /handshaking/syncing window, not just its first phase - exactly like
    // commandPending/demoPending. A ready device does not any more (see the
    // doc comment above), so it is deliberately not part of this guard. Note
    // this must read awaitingDetection rather than connectionRef.current's
    // phase: returnToDemoDevice's own transitional "discovering" state (set
    // right before it calls this function) is not a real handshake and must
    // not block it.
    if (commandPending || demoPending || awaitingDetection.current) return;
    const attempt = connectionAttempts.begin();
    const demoOpen = ++demoOpenToken.current;
    // A ready, verified Quad Cortex is left attached in the background rather
    // than closed: switchableGateway only changes which transport commands
    // are routed to, so its session stays open and resumeDeviceSession can
    // return to it later without repeating the handshake. A failed connect's
    // broker search is retained too: that search is what notices a QC plugged
    // in after startup. An explicit switch to Demo Mode may still close any
    // other incomplete real-device session.
    const keepDeviceInBackground = connectionRef.current.source === "device" && deviceReady;
    const closeDeviceSession = qcShouldDisconnectForDemo(connectionRef.current, deviceReady, returningFrom !== undefined);
    setDemoPending(true);
    stopSyncProgressAnimation();
    showSyncProgress(null);
    resetCommands();
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-starting", result: "pending", detail: "Opening the demo device saved on this computer." }]);
    try {
      if (closeDeviceSession) await tauriTransport.disconnect().catch(() => undefined);
      if (!attempt.current()) return;
      const opened = await demoDeviceSession.start(virtualDeviceStorage, attempt.current);
      if (!opened) return;
      const { transport: demo, connection: state, snapshot: current } = opened;
      clearUndo();
      presetWorkflow.resetCache();
      nativeStateAvailable.current = false;
      // The demo state first, then its content: the demo's preset is never on
      // screen under a device-sourced state, even between two updates.
      connectionRef.current = state;
      setConnection(state);
      setSnapshot(current);
      setSelectedBlockId("");
      setDeviceHeldInBackground(keepDeviceInBackground);
      setNotice(returningFrom
        ? `The Quad Cortex was not reached (${returningFrom}). Back on the demo device: ${current.presetLocation} · ${current.presetName}.`
        : keepDeviceInBackground
          ? `${state.detail} ${current.presetLocation} · ${current.presetName} loaded from this computer. The Quad Cortex stays connected in the background; select its USB tab to return to it.`
          : `${state.detail} ${current.presetLocation} · ${current.presetName} loaded from this computer.`);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: returningFrom ? "demo-device-restored" : "demo-device-started", result: "success", detail: keepDeviceInBackground ? "Demo device active. The Quad Cortex stays connected in the background; nothing is sent to it while the demo is active." : "Demo device active. Edits are saved on this computer; nothing is sent to a Quad Cortex." }]);
    } catch (error) {
      if (!attempt.current()) return;
      const detail = error instanceof Error ? error.message : String(error);
      if (keepDeviceInBackground) {
        // Nothing about the still-connected, still-ready Quad Cortex failed -
        // only opening the demo did - so the switch is simply abandoned
        // rather than replacing a healthy device state with a red error.
        setNotice(`The demo device could not be opened: ${detail}`);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-failed", result: "failure", detail }]);
        return;
      }
      const failed = { phase: "needs-attention" as const, detail: `The demo device could not be opened: ${detail}`, demo: true, source: "device" as const };
      // Returning from a failed reconnect, the Grid still shows "Connecting to
      // the Quad Cortex…"; that attempt is over, so it must not stay under the error.
      if (returningFrom) setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
      connectionRef.current = failed;
      setConnection(failed);
      setNotice(failed.detail);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-failed", result: "failure", detail }]);
    } finally {
      if (demoOpenToken.current === demoOpen) setDemoPending(false);
    }
  };

  /** "Stop the demo device": go fully idle - no demo, no hardware - keeping the demo's saved state. */
  const stopDemoDevice = () => {
    if (commandPending) {
      setNotice("Wait for the current command to finish before stopping the demo device.");
      return;
    }
    if (deviceHeldInBackground) {
      // A Quad Cortex stayed connected behind the demo (see startDemoDevice's
      // keepDeviceInBackground): "stop" has nothing truthful to go idle to,
      // since hardware is still attached and ready, so it resumes that device
      // instead of claiming to disconnect something that is still there.
      void resumeDeviceSession();
      return;
    }
    connectionAttempts.begin();
    // The demo's preset leaves the Grid with the demo itself, before the
    // device-sourced idle state is shown, so it is never paired with that
    // state or with the error of a later Connect that fails.
    if (suspendDemoDevice()) setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
    resetCommands();
    setDemoPending(false);
    const idle = { phase: "disconnected" as const, detail: "Demo device stopped. Its presets stay saved on this computer.", demo: true, source: "device" as const };
    connectionRef.current = idle;
    setConnection(idle);
    setNotice(idle.detail);
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-stopped", result: "info", detail: idle.detail }]);
  };

  /**
   * Switch back to a real Quad Cortex that stayed connected in the
   * background while the demo device was the active view (see
   * startDemoDevice's `keepDeviceInBackground`). No full reconnect: the
   * broker session was never closed, so this only re-verifies readiness and
   * re-reads the current snapshot once before swapping the routing target.
   */
  const resumeDeviceSession = async () => {
    if (!deviceHeldInBackground || commandPending || demoPending || demoDeviceSession.source !== "virtual") return;
    const attempt = connectionAttempts.begin();
    setCommandPending(true);
    setNotice("Reading the Quad Cortex that stayed connected in the background…");
    try {
      const verifiedRuntime = await tauriTransport.runtimeStatus();
      if (!attempt.current()) return;
      if (!runtimeHasSynchronizedQc(verifiedRuntime)) {
        throw new Error(verifiedRuntime.usbDiagnostics?.detail || "The Quad Cortex preset is not synchronized");
      }
      const current = await tauriTransport.currentSnapshot();
      if (!attempt.current()) return;
      demoDeviceSession.useDevice();
      setRuntime(verifiedRuntime);
      clearUndo();
      presetWorkflow.resetCache();
      nativeStateAvailable.current = false;
      setSnapshot(current);
      setSelectedBlockId("");
      const ready = { phase: "ready" as const, detail: "Quad Cortex connected.", demo: false, source: "device" as const, lastSync: new Date().toISOString() };
      connectionRef.current = ready;
      setConnection(ready);
      setDeviceHeldInBackground(false);
      setNotice(`Switched back to the Quad Cortex. ${current.presetLocation} · ${current.presetName} resumed from the background.`);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "device-resumed", result: "success", detail: "Switched back to the Quad Cortex that stayed connected in the background; no reconnect was needed." }]);
    } catch (error) {
      if (!attempt.current()) return;
      const detail = error instanceof Error ? error.message : String(error);
      // The background session did not actually survive: fall back exactly
      // like a failed reconnect from the demo would, rather than leaving a
      // USB tab on screen that cannot succeed.
      setDeviceHeldInBackground(false);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "device-resume-failed", result: "failure", detail }]);
      returnToDemoDevice(detail);
    } finally {
      if (attempt.current()) setCommandPending(false);
    }
  };

  /** The demo device's store, while the demo device is the active transport. */
  const activeDemoStore = () => demoDeviceSession.source === "virtual" ? virtualDeviceStoreOf(demoDeviceSession.virtual) : undefined;
  const readClipboardText = async (): Promise<string | undefined> => {
    try {
      return await navigator.clipboard.readText();
    } catch (error) {
      setNotice(`QC Remote could not read the clipboard: ${error instanceof Error ? error.message : String(error)}`);
      return undefined;
    }
  };

  /**
   * Edit > Copy Preset as Text. Works the same on either transport: blocks that
   * do not carry their values (all of them on real hardware) are read once
   * each, so the copy holds the active scene's values; a demo copy also carries
   * the demo's exact preset with every scene.
   */
  const copyPresetAsText = async () => {
    if (!sessionReady || commandPending) return;
    const current = snapshotRef.current;
    const store = activeDemoStore();
    setCommandPending(true);
    setNotice(`Copying ${current.presetName || "the preset"} as text…`);
    try {
      const copied = await capturePresetText({ gateway: activeGateway, snapshot: current, source: store ? "virtual" : "device", demoPreset: store?.currentPreset(), onProgress: setNotice });
      await navigator.clipboard.writeText(copied.text);
      setNotice(copied.summary);
    } catch (error) {
      setNotice(`The preset was not copied: ${error instanceof Error ? error.message : String(error)}`);
    } finally {
      setCommandPending(false);
    }
  };

  /**
   * Edit > Paste Preset from Text. Into the demo device it replaces the active
   * preset directly; onto a Quad Cortex it is replayed as ordinary block and
   * parameter writes, each guarded as usual, and every refused write is
   * reported. Either way the result is unsaved until the preset is saved.
   */
  const pastePresetFromText = async () => {
    if (!sessionReady || commandPending) return;
    const text = await readClipboardText();
    if (text === undefined) return;
    const parsed = parsePresetText(text);
    if (!parsed.ok) { setNotice(parsed.message); return; }
    const pasted = parsed.document;
    const pastedName = pasted.snapshot.presetName || "the unnamed preset";
    const target = snapshotRef.current;
    const occupied = target.blocks.filter(isGridDevice).length;
    const store = activeDemoStore();
    if (store) {
      if ((occupied || target.dirty) && !await appPrompts.confirm(`Replace “${target.presetName || "the unnamed preset"}” on the demo device with the pasted “${pastedName}”? Its ${occupied} device${occupied === 1 ? "" : "s"}${target.dirty ? " and unsaved changes" : ""} will be replaced. This is temporary until the preset is saved.`)) return;
      setCommandPending(true);
      try {
        const result = await pastePresetIntoDemo(store, pasted);
        clearUndo();
        reconcileWorkflowSnapshot(await activeGateway.currentSnapshot());
        setNotice(result.summary);
      } catch (error) {
        setNotice(`The preset was not pasted: ${error instanceof Error ? error.message : String(error)}`);
      } finally {
        setCommandPending(false);
      }
      return;
    }
    if (!deviceReady) return;
    // A paste is a full overwrite once confirmed. A copy from the demo device
    // always asks first, saying that its untouched values are approximations.
    const confirmation = presetReplayConfirmation(pasted, target);
    if (confirmation !== undefined && !await appPrompts.confirm(confirmation)) return;
    setCommandPending(true);
    try {
      const report = await replayPresetOnDevice({ gateway: activeGateway, document: pasted, target, isConnectionLost: isQcConnectionFailure, onProgress: setNotice });
      clearUndo();
      try { reconcileWorkflowSnapshot(await activeGateway.currentSnapshot()); } catch { if (report.snapshot) reconcileWorkflowSnapshot(report.snapshot); }
      setNotice(presetReplaySummary(report, pasted));
    } catch (error) {
      actionFailed(error);
    } finally {
      setCommandPending(false);
    }
  };

  /**
   * Edit > Copy Setlist records the authoritative source. A physical source is
   * also captured preset-by-preset so it can be pasted into the Demo device;
   * the firmware's atomic duplicate remains the fast path on the same QC.
   */
  const copySetlist = async () => {
    if (!sessionReady || commandPending) return;
    try {
      const identity = await activeGateway.identity();
      const source = connection.source === "virtual" ? "virtual" : "device";
      let captured: CapturedSetlist | undefined;
      if (source === "device") {
        if (!await appPrompts.confirm(`Copy all occupied presets in setlist “${snapshot.setlistName}”? QC Remote will briefly recall each preset and each of its eight scenes, read it, then restore ${snapshot.presetLocation} · ${snapshot.presetName}. Nothing on the Quad Cortex will be saved or overwritten.`)) return;
        setCommandPending(true);
        captured = await captureSetlistForDemo({
          gateway: activeGateway,
          setlistKey: snapshot.setlistKey,
          setlistName: snapshot.setlistName,
          onProgress: setNotice
        });
        reconcileWorkflowSnapshot(await activeGateway.currentSnapshot());
      }
      setSetlistClipboard({
        serial: identity.serial,
        source,
        setlistKey: snapshot.setlistKey,
        setlistName: snapshot.setlistName,
        captured
      });
      setNotice(`Copied setlist “${snapshot.setlistName}”${captured ? `: ${captured.presets.length} occupied presets and every scene were read` : ""}. Paste Setlist can duplicate it here${captured ? " or import it into the Demo device" : ""}.`);
    } catch (error) {
      actionFailed(error);
    } finally {
      setCommandPending(false);
    }
  };

  /** Edit > Paste Setlist uses native duplication on one device and reconstruction across device boundaries. */
  const pasteSetlist = async () => {
    if (!sessionReady || commandPending || !setlistClipboard) return;
    try {
      const identity = await activeGateway.identity();
      const source = connection.source === "virtual" ? "virtual" : "device";
      if (source === "virtual" && setlistClipboard.source === "device" && setlistClipboard.captured) {
        const store = activeDemoStore();
        if (!store) throw new Error("The Demo device store is not available.");
        if (!await appPrompts.confirm(`Import ${setlistClipboard.captured.presets.length} occupied presets from “${setlistClipboard.setlistName}” into the Demo device? They keep their original slot positions. Unsupported Capture or IR content will be reported and left out.`)) return;
        setCommandPending(true);
        const imported = await importCapturedSetlistIntoDemo(store, setlistClipboard.captured);
        presetWorkflow.resetCache();
        setNotice(`Imported “${imported.name}” into the Demo device with ${imported.presets} presets.${imported.skipped.length ? ` ${imported.skipped.length} unsupported block${imported.skipped.length === 1 ? " was" : "s were"} left out.` : " Every captured block was supported."}`);
        return;
      }
      if (identity.serial !== setlistClipboard.serial || source !== setlistClipboard.source) {
        setNotice("That setlist came from another target. Physical-to-Demo transfer is supported; writing a reconstructed setlist onto a different physical Quad Cortex is not enabled yet.");
        return;
      }
      if (!await appPrompts.confirm(`Duplicate setlist “${setlistClipboard.setlistName}” on this ${source === "virtual" ? "demo device" : "Quad Cortex"}? This creates a persistent copy.`)) return;
      setCommandPending(true);
      const result = await activeGateway.duplicateSetlist(setlistClipboard.setlistKey, setlistClipboard.setlistName);
      presetWorkflow.resetCache();
      setNotice(result.detail);
    } catch (error) {
      actionFailed(error);
    } finally {
      setCommandPending(false);
    }
  };

  const buildDiagnosticsReport = (): DiagnosticsReport => ({
    generatedAt: new Date().toISOString(),
    appVersion,
    runtime: { platform: runtime?.platform ?? "unknown", gatewayAvailable: runtime?.gatewayAvailable ?? false },
    connection: { phase: connection.phase, demo: connection.demo, source: connection.source },
    device: {
      presetLocation: snapshot.presetLocation,
      presetPosition: snapshot.presetPosition,
      mode: snapshot.mode,
      activeScene: snapshot.activeScene,
      tempo: snapshot.tempo,
      dirty: snapshot.dirty,
      blockCount: snapshot.blocks.length
    },
    events: connectionEvents
  });

  // The clipboard path for the Feedback window: the same report the export
  // writes, as text to paste into the issue form's diagnostics field. It is
  // handed to the Feedback window rather than written here, because the
  // clipboard refuses a write from a window that does not have focus.
  const diagnosticsText = async (): Promise<string> => JSON.stringify(buildDiagnosticsReport(), null, 2);

  // Returns what happened rather than announcing it. The Feedback window is a
  // separate window with its own message line, so the outcome — cancelled,
  // saved, or failed — has to travel back to where its reader is looking; the
  // connection panel announces the same string itself.
  const exportDiagnostics = async (): Promise<string> => {
    const result = await diagnosticsFiles.export(buildDiagnosticsReport());
    if (result.cancelled) return "Diagnostics export cancelled.";
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "diagnostics-exported", result: "success", detail: "A redacted diagnostic report was exported." }]);
    return `Redacted diagnostics exported as ${result.name}.`;
  };

  // The Feedback window asks for diagnostics over the event bus, because the
  // report is built from this window's live state. The handlers are read
  // through a ref so the listeners are attached once, for the life of the
  // window, and always build the report from the latest render.
  const diagnosticsHandlers = useRef({ exportDiagnostics, diagnosticsText });
  diagnosticsHandlers.current = { exportDiagnostics, diagnosticsText };
  useEffect(() => serveDiagnosticsRequests({
    exportReport: () => diagnosticsHandlers.current.exportDiagnostics(),
    reportText: () => diagnosticsHandlers.current.diagnosticsText()
  }), []);

  useEffect(() => {
    if (autoConnectStarted.current) return;
    autoConnectStarted.current = true;
    // Demo Mode is the app's default: it is what the very first frame is
    // usable against, not "Connect the Quad Cortex" placeholders. Starting it
    // here, awaited before connect() runs, guarantees switchableGateway is
    // already routed to it (source "virtual") by the time connect() makes its
    // own `fromDemo = suspendDemoDevice()` check - so the very first connect
    // attempt takes the same already-tested suspend-on-success /
    // restore-on-failure path a later reconnect from the demo already uses,
    // rather than a second, bespoke launch sequence. If the demo itself
    // cannot be opened, connect() still runs against real hardware exactly as
    // before.
    void startDemoDevice().finally(() => connect());
    return () => { autoConnectStarted.current = false; };
  }, []);

  // The broker's own phase is the evidence a device is there. It is pushed,
  // because device.reconnect holds the gateway for the whole handshake and
  // nothing can be asked of it meanwhile. Read through a ref so a detection
  // long after mount still runs the current render's connect.
  const connectOnDetection = useRef(connect);
  connectOnDetection.current = connect;
  useEffect(() => onDevicePhase(({ phase, detail, deviceId }) => {
    if (selectedPhysicalRef.current && deviceId !== selectedPhysicalRef.current) return;
    if (!brokerPhaseDetectsDevice(phase)) return;
    if (awaitingDetection.current) {
      beginSyncProgress();
      const detected = qcConnectionDuringBrokerPhase(phase, detail);
      if (detected && (connectionRef.current.phase !== detected.phase || connectionRef.current.detail !== detected.detail)) {
        connectionRef.current = detected;
        setConnection(detected);
        setNotice(detected.detail);
        setSnapshot({
          ...CONNECTING_PLACEHOLDER_SNAPSHOT,
          presetName: detected.phase === "opening"
            ? "Opening the Quad Cortex…"
            : detected.phase === "handshaking"
              ? "Handshaking with the Quad Cortex…"
              : "Synchronizing the Quad Cortex…"
        });
      }
      return;
    }
    // Plugged in after a connect had already given up: the broker keeps
    // looking on its own, so follow it rather than waiting for a click.
    if (qcShouldConnectOnDetection(connectionRef.current)) void connectOnDetection.current();
  }), []);

  useEffect(() => {
    const recovering = connection.phase === "needs-attention" && connection.detail.startsWith("Live synchronization");
    if (!((connection.phase === "ready" && !connection.demo && connection.source === "device") || recovering)) return;
    if (!recovering && nativeStateAvailable.current) return;
    let cancelled = false;
    let timer: number | undefined;
    const schedule = (delay = recovering ? 500 : 250) => {
      if (!recovering && nativeStateAvailable.current) return;
      if (!cancelled) timer = window.setTimeout(() => void synchronize(), delay);
    };
    const synchronize = async () => {
      // Sparse Grid repair is event-triggered above. Do not let the live-sync
      // fallback turn a failed repair into a type-15 polling loop.
      if (snapshotRef.current.gridLayoutComplete === false) return;
      if (document.visibilityState !== "visible" || commandPending || parameterWorkflow.hasPendingChanges() || presetListLoading || presetFoldersLoading) {
        schedule();
        return;
      }
      try {
        let verifiedRuntime = runtime;
        const recovered = recovering || liveSyncFailures.current >= 2;
        const current = await qcController.readSnapshot(async () => {
          const incoming = await tauriTransport.currentSnapshot();
          verifiedRuntime = recovering ? await tauriTransport.runtimeStatus() : runtime;
          if (cancelled) throw new SupersededPresetOperation();
          if (recovering && !runtimeHasSynchronizedQc(verifiedRuntime)) {
            throw new Error(verifiedRuntime?.usbDiagnostics?.detail || "The Quad Cortex preset is not synchronized");
          }
          // Preserve newer volume samples delivered by the native event stream.
          return {...incoming, masterVolume: recovered ? incoming.masterVolume : snapshotRef.current.masterVolume};
        });
        if (cancelled) return;
        if (verifiedRuntime) setRuntime(verifiedRuntime);
        liveSyncFailures.current = 0;
        setSelectedBlockId((selected) => current.blocks.some((block) => block.id === selected) ? selected : "");
        if (recovered) {
          const detail = "Quad Cortex reconnected automatically; live state synchronized.";
          const ready = { ...connectionRef.current, phase: "ready" as const, demo: false, detail, lastSync: new Date().toISOString() };
          connectionRef.current = ready;
          setConnection(ready);
          setNotice(detail);
          setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "live-sync-recovered", result: "success", detail }]);
        }
      } catch (error) {
        if (cancelled) return;
        if (error instanceof SupersededPresetOperation) { schedule(); return; }
        if (isIncompleteGridRefreshFailure(error)) {
          qcController.updateSnapshot((current) => current.gridLayoutComplete === false
            ? current
            : {...current, gridLayoutComplete: false});
          setNotice(error instanceof Error ? error.message : String(error));
          return;
        }
        liveSyncFailures.current += 1;
        if (liveSyncFailures.current === 2) {
          const detail = error instanceof Error ? error.message : String(error);
          beginLiveRecovery(detail);
        }
      }
      schedule(liveSyncFailures.current >= 2 ? 500 : 250);
    };
    schedule(250);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [beginLiveRecovery, commandPending, connection.demo, connection.phase, connection.source, presetFoldersLoading, presetListLoading, snapshot.gridLayoutComplete]);

  const refreshSnapshot = async () => {
    if (!deviceReady || commandPending) return;
    const origin = qcController.lifetime.checkpoint();
    setCommandPending(true);
    setNotice("Refreshing complete device state…");
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-started", result: "pending", detail: "Reading the complete active preset state from the device." }]);
    try {
      const current = await qcController.readSnapshot(() => tauriTransport.currentSnapshot());
      setConnection((state) => ({ ...state, lastSync: new Date().toISOString() }));
      setNotice("Live preset state refreshed.");
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-complete", result: "success", detail: `${current.presetLocation} · ${current.presetName} refreshed successfully.` }]);
    } catch (error) {
      if (error instanceof SupersededPresetOperation) return;
      const detail = error instanceof Error ? error.message : String(error);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-failed", result: "failure", detail }]);
      actionFailed(error);
    } finally {
      if (qcController.lifetime.current(origin)) setCommandPending(false);
    }
  };

  const undoLastAction = async () => {
    if (undoEntry) return deviceHistory.undo();
    if (!deviceUndoAvailable || !snapshot.presetIdentity) return;
    try { await activeGateway.undo(snapshot.presetIdentity); }
    catch (error) { actionFailed(error); }
  };
  const redoLastAction = async () => {
    if (redoEntry) return deviceHistory.redo();
    if (!deviceRedoAvailable || !snapshot.presetIdentity) return;
    try { await activeGateway.redo(snapshot.presetIdentity); }
    catch (error) { actionFailed(error); }
  };

  const saveWorkspace = async (saveAs = false): Promise<{ ok: boolean; message: string }> => {
    if (commandPending) return { ok: false, message: "Another operation is still running. Try again when it finishes." };
    setCommandPending(true);
    try {
      // The document is built and written inside saveWorkspaceDocument; its snapshot never carries the
      // device's model preset list.
      const { document, result } = await saveWorkspaceDocument(workspaceFiles, {
        snapshot,
        selectedBlock: blockDetails,
        selectedBlockId,
        formFactorId,
        path: workspacePath,
        saveAs,
        suggestedName: `${snapshot.presetLocation} ${snapshot.presetName}`
      });
      if (!result.cancelled && result.path) {
        setWorkspacePath(result.path);
        setWorkspaceName(result.name);
        setLoadedWorkspace(document);
        const saved = `Workspace saved: ${result.name}`;
        setNotice(saved);
        return { ok: true, message: saved };
      }
      return { ok: true, message: "Save cancelled. No copy was written." };
    } catch (error) {
      actionFailed(error);
      return { ok: false, message: error instanceof Error ? error.message : String(error) };
    } finally {
      setCommandPending(false);
    }
  };

  const createDeviceBackup = async () => {
    if (!deviceReady || commandPending) return;
    const today = new Date().toISOString().slice(0, 10);
    const name = (await appPrompts.prompt("Backup name", `QC Device Backup ${today}`))?.trim();
    if (!name) return;
    setCommandPending(true);
    setNotice("Choose where to save the native backup. The device transfer can take about 40 seconds.");
    try {
      const result = await tauriTransport.createDeviceBackup(name);
      setNotice(result.cancelled ? "Device backup cancelled." : `Native Quad Cortex backup saved as ${result.name}.`);
    } catch (error) {
      actionFailed(error);
    } finally {
      setCommandPending(false);
    }
  };

  const openWorkspace = async () => {
    if (commandPending) return;
    setCommandPending(true);
    try {
      const result = await workspaceFiles.open();
      if (!result.cancelled && result.document && result.path) {
        setWorkspacePath(result.path);
        setWorkspaceName(result.name);
        setLoadedWorkspace(result.document);
        void appWindows.open("workspace").catch(actionFailed);
        setNotice(`Workspace opened: ${result.name}`);
      }
    } catch (error) {
      actionFailed(error);
    } finally {
      setCommandPending(false);
    }
  };

  // The one place an AI access level change is applied and remembered: the
  // LOCAL panel's header control and the Settings window both land here.
  const changeControlAccessMode = (mode: ControlAccessMode) => {
    setControlAccessMode(mode);
    writeControlAccessMode(localStorage, mode);
    setNotice(`AI access level changed to ${mode}.`);
  };
  const changeFollowQcScreen = (enabled: boolean) => {
    setFollowQcScreen(enabled);
    writeQcScreenFollowSetting(localStorage, enabled);
    setNotice(enabled ? "Following supported Quad Cortex screen changes while QC Remote is visible." : "Quad Cortex screen following is off.");
  };

  // What the Settings, Device Info and Workspace windows show. Each is a
  // separate webview with no access to this window's state, so it is sent to
  // them over the event bus: once when a window asks, and again whenever one
  // of these values changes. The memos depend on the displayed fields only,
  // so a device frame that changes none of them sends nothing.
  const deviceConnectionLabel = deviceReady ? "ready" : connection.phase === "ready" ? "verifying" : connection.phase;
  const deviceInfoState = useMemo<DeviceInfoState>(() => ({
    deviceName: snapshot.deviceName,
    connection: deviceConnectionLabel,
    setlistName: snapshot.setlistName,
    presetLocation: snapshot.presetLocation,
    presetName: snapshot.presetName,
    mode: snapshot.mode,
    scene: sceneLetter(snapshot.activeScene),
    tempo: snapshot.tempo,
    blockCount: snapshot.blocks.length,
    dirty: snapshot.dirty
  }), [snapshot.deviceName, deviceConnectionLabel, snapshot.setlistName, snapshot.presetLocation, snapshot.presetName, snapshot.mode, snapshot.activeScene, snapshot.tempo, snapshot.blocks.length, snapshot.dirty]);
  const workspaceState = useMemo<WorkspaceState>(() => loadedWorkspace ? {
    name: workspaceName ?? "QC Workspace",
    savedAt: loadedWorkspace.savedAt,
    setlistName: loadedWorkspace.source.setlistName,
    presetLocation: loadedWorkspace.source.presetLocation,
    presetName: loadedWorkspace.source.presetName,
    scene: sceneLetter(loadedWorkspace.snapshot.activeScene),
    blockCount: loadedWorkspace.snapshot.blocks.length,
    dirty: loadedWorkspace.snapshot.dirty
  } : null, [loadedWorkspace, workspaceName]);
  const [brokerTrayStatus, setBrokerTrayStatus] = useState<BrokerTrayStatus>(DEFAULT_BROKER_TRAY_STATUS);
  const trayStatusReadVersion = useRef(0);
  const trayChangePending = useRef(false);
  const refreshBrokerTray = useCallback(async () => {
    if (trayChangePending.current) return;
    const version = trayStatusReadVersion.current;
    try {
      const status = await brokerTray.status();
      if (version === trayStatusReadVersion.current && !trayChangePending.current) setBrokerTrayStatus(status);
    } catch (error) {
      if (version === trayStatusReadVersion.current && !trayChangePending.current) {
        setBrokerTrayStatus((current) => ({ ...current, lastError: error instanceof Error ? error.message : String(error) }));
      }
    }
  }, []);
  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      await refreshBrokerTray();
      if (!disposed) timer = setTimeout(() => void poll(), 1_000);
    };
    void poll();
    return () => { disposed = true; trayStatusReadVersion.current += 1; clearTimeout(timer); };
  }, [refreshBrokerTray]);
  const applyBrokerTrayChange = useCallback(async (change: () => Promise<BrokerTrayStatus>) => {
    trayChangePending.current = true;
    trayStatusReadVersion.current += 1;
    try { return await changeBrokerTray(change, setBrokerTrayStatus); }
    finally { trayChangePending.current = false; trayStatusReadVersion.current += 1; }
  }, []);
  const settingsState = useMemo<SettingsState>(() => ({
    ...brokerTrayStatus,
    formFactorId,
    formFactors: formFactors.map((item) => ({ id: item.id, displayName: item.displayName })),
    controlAccessMode,
    followQcScreen,
    localAccessCondition: localAccessCondition ? { tone: localAccessCondition.tone, message: localAccessCondition.message, detail: localAccessCondition.detail } : null
  }), [brokerTrayStatus, formFactorId, controlAccessMode, followQcScreen, localAccessCondition?.tone, localAccessCondition?.message, localAccessCondition?.detail]);
  useEffect(() => publishWindowState(DEVICE_INFO_EVENTS, deviceInfoState), [deviceInfoState]);
  useEffect(() => publishWindowState(WORKSPACE_EVENTS, workspaceState), [workspaceState]);
  useEffect(() => publishWindowState(SETTINGS_EVENTS, settingsState), [settingsState]);

  // Read through a ref, like the diagnostics handlers, so the listeners are
  // attached once and always see the latest render. A Settings change is
  // applied by the same setters the in-app controls call, after checking it
  // names a real device model or access level: an event payload is not typed.
  const appWindowHandlers = useRef({ deviceInfoState, workspaceState, settingsState, changeControlAccessMode, changeFollowQcScreen, saveWorkspace });
  appWindowHandlers.current = { deviceInfoState, workspaceState, settingsState, changeControlAccessMode, changeFollowQcScreen, saveWorkspace };
  useEffect(() => serveAppWindowRequests({
    deviceInfoState: () => appWindowHandlers.current.deviceInfoState,
    workspaceState: () => appWindowHandlers.current.workspaceState,
    settingsState: () => appWindowHandlers.current.settingsState,
    setFormFactor: async (formFactorId) => {
      const model = formFactors.find((item) => item.id === formFactorId);
      if (!model) throw new Error(`${String(formFactorId)} is not a device model this build knows.`);
      setFormFactorId(model.id);
      return `Device model changed to ${model.displayName}.`;
    },
    setControlAccessMode: async (mode) => {
      const option = CONTROL_ACCESS_OPTIONS.find((entry) => entry.value === mode);
      if (!option) throw new Error(`${String(mode)} is not an AI access level.`);
      appWindowHandlers.current.changeControlAccessMode(option.value);
      return `AI access level changed to ${option.label}.`;
    },
    setFollowQcScreen: async (enabled) => {
      if (typeof enabled !== "boolean") throw new Error("Follow QC screen must be on or off.");
      appWindowHandlers.current.changeFollowQcScreen(enabled);
      return enabled ? "QC screen following enabled." : "QC screen following disabled.";
    },
    setBrokerTrayEnabled: async (enabled) => {
      const status = await applyBrokerTrayChange(() => brokerTray.setEnabled(enabled));
      return status.trayStopping ? "Broker tray stopping; it will exit within five seconds."
        : status.trayEnabled ? "Tray icon shown." : "Tray icon hidden.";
    },
    setIdleDisconnectMinutes: async (minutes) => {
      const status = await applyBrokerTrayChange(() => brokerTray.setIdleMinutes(minutes));
      return status.idleDisconnectMinutes === null
        ? "Release set to Never. The device stays connected until you choose Disconnect or Quit."
        : status.idleDisconnectMinutes === 0
          ? "Release set to Immediately, about five seconds after the last attached app or AI client disconnects."
          : `Release set to ${status.idleDisconnectMinutes} minutes after the last attached app or AI client disconnects.`;
    },
    saveWorkspaceCopy: async () => {
      const outcome = await appWindowHandlers.current.saveWorkspace(true);
      if (!outcome.ok) throw new Error(outcome.message);
      return outcome.message;
    }
  }), []);

  const exitApp = async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
  };

  /**
   * Report a LOCAL result once, in the app's own live region.
   *
   * Each result used to be kept in a live region inside the provider card as
   * well, where it stayed until another action replaced it - so six rows
   * accumulated six paragraphs and the panel read as a log. The status strip is
   * `role="status"` too, so a failure is still announced and still visible; it
   * just does not become part of the row.
   */
  const reportLocal = useCallback((outcome: LocalActionOutcome) => {
    setNotice(outcome.message);
  }, []);

  /** Announce that a LOCAL action has started. */
  const beginLocal = useCallback((message: string) => {
    setNotice(message);
  }, []);

  /**
   * The failure sentence may only name a control the user can see.
   *
   * `retryLabel` defaulted to "Pair", and the handlers that fail most often
   * belong to rows that are already paired - whose two buttons are their
   * primary and Unpair. So a failed Update told the user to press a button
   * that was not on the row. Every caller now passes the label of the control
   * it was invoked from, read off the same presentation the row draws where
   * that is a row button, and nothing at all where the act came from a menu
   * entry the row does not carry.
   */
  const failLocal = useCallback((app: string, error: unknown, retryLabel?: string) => {
    reportLocal(localActionFailure(app, error, retryLabel));
  }, [reportLocal]);

  /** The word on each row's primary right now, for the two uses above. */
  const claudeRetryLabel = () => claudeExtensionPresentation(claudeExtensionStatus, claudeInstallPending, claudeStatusRefreshFailed, claudeApprovalLapsed).primary?.label;
  const chatgptRetryLabel = () => chatgptIntegrationPresentation(chatgptIntegrationStatus, chatgptRestartRequired, chatgptStatusRefreshFailed, chatgptRemovalRestartRequired).primary?.label;
  const antigravityRetryLabel = () => antigravityIntegrationPresentation(antigravityIntegrationStatus, antigravityStatusRefreshFailed).primary?.label;
  /**
   * The name every LM Studio sentence in this file uses, read off the
   * installed build rather than written down - see `lmStudioAppName`. The row
   * in `menu-bar.tsx` derives it from the same status document, so a notice
   * this file reports and the row it came from cannot name two applications.
   */
  const lmStudioName = lmStudioAppName(lmStudioIntegrationStatus);
  /**
   * The switch this installation still leaves for the owner, or nothing at
   * all. Read off the same status document the row is, so a notice this file
   * reports cannot send him to a screen the row says he does not need.
   */
  const lmStudioSwitch = lmStudioEnableSwitch(lmStudioIntegrationStatus);
  const lmStudioRetryLabel = () => managedMcpIntegrationPresentation(lmStudioName, lmStudioIntegrationStatus, Boolean(lmStudioIntegrationStatus?.lmStudioInstalled), lmStudioSwitch, lmStudioStatusRefreshFailed).primary?.label;
  const janRetryLabel = () => managedMcpIntegrationPresentation("Jan", janIntegrationStatus, Boolean(janIntegrationStatus?.janInstalled), "Settings → MCP Servers", janStatusRefreshFailed).primary?.label;

  /**
   * The one Claude status read. It returns the status it just stored as well as
   * whether the read worked, because a repair has to report what Claude says
   * *now* - `claudeExtensionStatus` still holds the value React rendered before
   * the click.
   */
  const refreshClaudeExtensionStatus = useCallback(async (finishPending = false) => {
    if (!window.__TAURI_INTERNALS__) return { ok: true } as const;
    try {
      const nextStatus = await localClientPackages.claudeDesktopStatus();
      setClaudeExtensionStatus(nextStatus);
      if (nextStatus.state === "current" || nextStatus.state === "newer-installed") setClaudeApprovalLapsed(false);
      if (finishPending || nextStatus.state === "current" || nextStatus.state === "newer-installed") {
        setClaudeInstallPending(false);
      }
      return { ok: true, status: nextStatus } as const;
    } catch {
      return { ok: false } as const;
    }
  }, []);

  const refreshChatgptIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      const nextStatus = await localClientPackages.chatgptStatus();
      setChatgptIntegrationStatus(nextStatus);
      setChatgptRestartRequired(nextStatus.restartRequired);
      if (!nextStatus.chatgptRunning) setChatgptRemovalRestartRequired(false);
      return true;
    } catch {
      return false;
    }
  }, []);

  const refreshAntigravityIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      const nextStatus = await localClientPackages.antigravityStatus();
      setAntigravityIntegrationStatus(nextStatus);
      return true;
    } catch {
      return false;
    }
  }, []);

  const refreshLmStudioIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      setLmStudioIntegrationStatus(await localClientPackages.lmStudioStatus());
      return true;
    } catch {
      return false;
    }
  }, []);

  const refreshJanIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      setJanIntegrationStatus(await localClientPackages.janStatus());
      return true;
    } catch {
      return false;
    }
  }, []);

  /**
   * The state of the loopback surface itself, as distinct from any one
   * pairing. A second copy of QC Remote owning the published connection is
   * invisible in every provider row, because no row knows who is serving.
   */
  const refreshLocalAccessStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      setLocalAccessStatus(await localAccess.status());
      return true;
    } catch {
      // An unreadable status is not itself a fault to shout about: the panel
      // simply says nothing rather than inventing a state.
      setLocalAccessStatus(undefined);
      return false;
    }
  }, []);

  const refreshOtherMcpStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    try {
      setOtherMcpStatus(await localClientPackages.otherMcpStdioSetup());
      return true;
    } catch {
      return false;
    }
  }, []);

  const refreshLocalIntegrations = useCallback(async (finishClaudePending = false) => {
    if (localStatusRefreshInFlight.current) return;
    localStatusRefreshInFlight.current = true;
    setLocalStatusChecking(true);
    try {
      const [claudeRead, chatgptOk, antigravityOk, lmStudioOk, janOk, otherOk] = await Promise.all([
        refreshClaudeExtensionStatus(finishClaudePending),
        refreshChatgptIntegrationStatus(),
        refreshAntigravityIntegrationStatus(),
        refreshLmStudioIntegrationStatus(),
        refreshJanIntegrationStatus(),
        refreshOtherMcpStatus(),
        refreshLocalAccessStatus()
      ]);
      setClaudeStatusRefreshFailed(!claudeRead.ok);
      setChatgptStatusRefreshFailed(!chatgptOk);
      setAntigravityStatusRefreshFailed(!antigravityOk);
      setLmStudioStatusRefreshFailed(!lmStudioOk);
      setJanStatusRefreshFailed(!janOk);
      setOtherMcpStatusRefreshFailed(!otherOk);
    } finally {
      setLocalStatusChecking(false);
      localStatusRefreshInFlight.current = false;
    }
  }, [refreshAntigravityIntegrationStatus, refreshChatgptIntegrationStatus, refreshClaudeExtensionStatus, refreshJanIntegrationStatus, refreshLmStudioIntegrationStatus, refreshLocalAccessStatus, refreshOtherMcpStatus]);

  useEffect(() => {
    void refreshLocalIntegrations();
    const refreshOnFocus = () => window.setTimeout(() => void refreshLocalIntegrations(true), 350);
    window.addEventListener("focus", refreshOnFocus);
    return () => window.removeEventListener("focus", refreshOnFocus);
  }, [refreshLocalIntegrations]);

  useEffect(() => {
    if (!localPanelOpen) return;
    void refreshLocalIntegrations(true);
    const refreshTimer = window.setInterval(() => void refreshLocalIntegrations(true), 5_000);
    return () => window.clearInterval(refreshTimer);
  }, [localPanelOpen, refreshLocalIntegrations]);

  useEffect(() => {
    if (!claudeInstallPending) return;
    const refreshTimer = window.setInterval(() => void refreshClaudeExtensionStatus(), 1_500);
    // Two minutes of polling used to end in silence: the flag cleared and the
    // row slid back to "Available", as though nothing had ever been asked for.
    const stopTimer = window.setTimeout(() => {
      setClaudeInstallPending(false);
      setClaudeApprovalLapsed(true);
      reportLocal({
        tone: "action-required",
        message: "Claude did not confirm the QC Remote extension. Open Claude Desktop and approve it, or press Pair again."
      });
    }, 120_000);
    return () => {
      window.clearInterval(refreshTimer);
      window.clearTimeout(stopTimer);
    };
  }, [claudeInstallPending, refreshClaudeExtensionStatus, reportLocal]);

  /**
   * The one Claude repair, reached by Pair and by every Update the row shows.
   *
   * `install_claude_desktop_extension` writes the .mcpb bundle and spawns
   * Claude with it, which is what makes Claude show its extension page with QC
   * Remote selected and an Install or Reinstall control on it. Re-running that
   * is what QC Remote can actually do about a pairing that is not working; the
   * row used to send the already-paired case to "open Claude Desktop" instead,
   * which is why pressing Update produced another sentence and no repair.
   *
   * A first install keeps the approval poll. Updating or reinstalling an
   * extension that is already installed reads the status straight back, so the
   * row reports whether Claude has actually connected. Only Claude can open
   * the MCP connection, and it does that on its own restart; the row's own
   * detail already says so, so a disconnected result does not repeat the
   * approval instruction.
   */
  const installClaudeDesktopExtension = async () => {
    if (localIntegrationAction) return;
    const updatingInstalledExtension = Boolean(claudeExtensionStatus?.installedVersion);
    setLocalIntegrationAction("claude");
    setClaudeApprovalLapsed(false);
    beginLocal(updatingInstalledExtension
      ? "Updating the QC Remote extension in Claude Desktop…"
      : "Pairing Claude Desktop…");
    try {
      await localClientPackages.installClaudeDesktop();
      if (!updatingInstalledExtension) {
        setClaudeInstallPending(true);
        reportLocal({
          tone: "action-required",
          message: "Claude opened the QC Remote installer. Approve it there — choose Always allow for QC Remote tools to avoid repeated prompts. LOCAL detects it automatically."
        });
        return;
      }
      const recheck = await refreshClaudeExtensionStatus();
      reportLocal(recheck.status?.connectionActive
        ? { tone: "ready", message: "Claude Desktop is connected to QC Remote." }
        : { tone: "action-required", message: "The QC Remote extension was reinstalled in Claude Desktop. Claude opens the connection itself, and this row turns green as soon as it does." });
    } catch (error) {
      failLocal("Claude Desktop", error, claudeRetryLabel());
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  /**
   * Claude's own extension page, opened with QC Remote selected.
   *
   * Removing an extension belongs to Claude, so putting the user in front of
   * that control is the whole of what Unpair can do here - and it is the same
   * native call the repair makes, not a second mechanism. The panel printed a
   * numbered list of places to click instead.
   */
  const openClaudeExtensionsPage = async () => {
    try {
      await localClientPackages.installClaudeDesktop();
      return true;
    } catch (error) {
      failLocal("Claude Desktop", error, "Unpair");
      return false;
    }
  };

  const openClaudeDesktopDownload = async () => {
    try {
      await localClientPackages.openClaudeDesktopDownload();
      reportLocal({ tone: "action-required", message: "Claude Desktop's download page opened. Install it for this Windows account, then press Refresh here." });
    } catch (error) {
      failLocal("Claude Desktop", error, "Install Claude Desktop");
    }
  };

  const openClaudeDesktop = async () => {
    try {
      await localClientPackages.openClaudeDesktop();
      reportLocal({ tone: "ready", message: "Claude Desktop opened. Start a new chat; choose Always allow for QC Remote tools to avoid repeated prompts." });
      await refreshClaudeExtensionStatus();
    } catch (error) {
      failLocal("Claude Desktop", error, "Open");
    }
  };

  const installChatgptLocalIntegration = async () => {
    if (localIntegrationAction) return;
    const retry = chatgptRetryLabel();
    setLocalIntegrationAction("chatgpt");
    beginLocal("Pairing ChatGPT / Codex…");
    try {
      const result = await localClientPackages.installChatgpt();
      await refreshChatgptIntegrationStatus();
      setChatgptRemovalRestartRequired(false);
      // installed, skillInstalled and skillBackupPath have always been in this
      // result and the panel read none of them: a half-finished pairing, or a
      // pre-existing skill moved aside, was announced as plain success.
      reportLocal(localPairOutcome("ChatGPT", {
        installed: result.installed,
        skillInstalled: result.skillInstalled,
        skillBackupPath: result.skillBackupPath,
        skillError: result.skillError,
        // ChatGPT spawns its MCP servers per chat session, so the next chat
        // is when this lands - there is no restart to ask anybody for, and
        // the branch that asked for one is gone.
        nextStep: `QC Remote and QC Device Knowledge ${result.skillVersion} are installed. Open a new chat in ChatGPT to use them.`
      }, retry));
    } catch (error) {
      failLocal("ChatGPT", error, retry);
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const removeChatgptLocalIntegration = async () => {
    if (localIntegrationAction) return false;
    setLocalIntegrationAction("remove-chatgpt");
    beginLocal("Unpairing ChatGPT / Codex…");
    try {
      const result = await localClientPackages.removeChatgpt();
      setChatgptRemovalRestartRequired(result.restartRequired);
      await refreshChatgptIntegrationStatus();
      // The backend removes the skill this app installed and restores any copy
      // it displaced (remove_device_knowledge_skill_at); say what it did.
      const skillNote = result.skillRemoved
        ? result.skillRestoredPath
          ? " The QC Device Knowledge skill it installed was removed and your previous copy restored."
          : " The QC Device Knowledge skill it installed was removed."
        : "";
      reportLocal({
        tone: result.restartRequired ? "action-required" : "ready",
        message: result.restartRequired
          ? `QC Remote was unpaired from ChatGPT and Codex. Restart ChatGPT once to disconnect any chat already open.${skillNote}`
          : `QC Remote was unpaired from ChatGPT and Codex. Other MCP servers were not changed.${skillNote}`
      });
      return true;
    } catch (error) {
      failLocal("ChatGPT", error, "Unpair");
      return false;
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const openChatgptDesktop = async () => {
    try {
      await localClientPackages.openChatgpt();
      // One sentence in every case. The branch this replaces walked the user
      // through ChatGPT's settings to find a Restart control, which does not
      // exist and which nothing here needs: ChatGPT spawns its MCP servers
      // per chat session, so a new chat is the whole of what makes a
      // rewritten route take effect.
      reportLocal({ tone: "ready", message: "ChatGPT Desktop opened. Start a new chat and type /mcp to confirm QC Remote is connected." });
      await refreshChatgptIntegrationStatus();
    } catch (error) {
      failLocal("ChatGPT", error, "Open");
    }
  };

  const openChatgptDesktopDownload = async () => {
    try {
      await localClientPackages.openChatgptDownload();
      reportLocal({ tone: "action-required", message: "The official ChatGPT Desktop download page opened. Install it for this Windows account, then press Refresh here." });
    } catch (error) {
      failLocal("ChatGPT", error, "Install ChatGPT Desktop");
    }
  };

  const openChatgptConfiguration = async () => {
    try {
      await localClientPackages.openChatgptConfiguration();
      reportLocal({ tone: "action-required", message: "The shared ChatGPT and Codex MCP configuration is selected. Fix config.toml, then press Refresh here." });
    } catch (error) {
      failLocal("ChatGPT", error);
    }
  };

  const openAntigravityDownload = async () => {
    try {
      await localClientPackages.openAntigravityDownload();
      reportLocal({ tone: "action-required", message: "The official Google Antigravity download page opened. Install it for this Windows account, then press Refresh here." });
    } catch (error) {
      failLocal("Google Antigravity", error, "Install Antigravity");
    }
  };

  const installAntigravityLocalIntegration = async () => {
    if (localIntegrationAction) return;
    const retry = antigravityRetryLabel();
    setLocalIntegrationAction("antigravity");
    beginLocal("Pairing Google Antigravity…");
    try {
      const result = await localClientPackages.installAntigravity();
      await refreshAntigravityIntegrationStatus();
      reportLocal(localPairOutcome("Google Antigravity", {
        installed: result.installed,
        skillInstalled: result.skillInstalled,
        skillBackupPath: result.skillBackupPath,
        skillError: result.skillError,
        nextStep: result.connectionActive
          ? "QC Remote is paired and connected to Google Antigravity."
          : result.refreshSucceeded
            ? "Google Antigravity took the QC Remote connection. It opens it when it next needs it."
            : "QC Remote is paired with Google Antigravity. Open Antigravity to start the connection."
      }, retry));
    } catch (error) {
      failLocal("Google Antigravity", error, retry);
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const removeAntigravityLocalIntegration = async () => {
    if (localIntegrationAction) return false;
    setLocalIntegrationAction("remove-antigravity");
    beginLocal("Unpairing Google Antigravity…");
    try {
      await localClientPackages.removeAntigravity();
      await refreshAntigravityIntegrationStatus();
      reportLocal({ tone: "ready", message: "QC Remote was unpaired from Google Antigravity. Other MCP servers were not changed." });
      return true;
    } catch (error) {
      failLocal("Google Antigravity", error, "Unpair");
      return false;
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const openAntigravity = async () => {
    try {
      await localClientPackages.openAntigravity();
      reportLocal({ tone: "ready", message: "Google Antigravity opened. Start a conversation and ask about your QC." });
      await refreshAntigravityIntegrationStatus();
    } catch (error) {
      failLocal("Google Antigravity", error, "Open");
    }
  };

  const installLmStudioLocalIntegration = async () => {
    if (localIntegrationAction) return;
    const retry = lmStudioRetryLabel();
    setLocalIntegrationAction("lm-studio");
    beginLocal(`Pairing ${lmStudioName}…`);
    try {
      const result = await localClientPackages.installLmStudio();
      await refreshLmStudioIntegrationStatus();
      reportLocal(localPairOutcome(lmStudioName, {
        installed: result.installed,
        // `installed` reports the MCP route write alone, so a skill that
        // failed was announced as a completed pair and the row then asked for
        // an Update that said success and changed nothing.
        skillInstalled: result.skillInstalled,
        skillBackupPath: result.skillBackupPath,
        skillError: result.skillError,
        nextStep: lmStudioSwitch
          ? `QC Remote is paired with ${lmStudioName}. Open ${lmStudioSwitch} in ${lmStudioName} and enable QC Remote.`
          : `QC Remote is paired with ${lmStudioName} and enabled. Start a new chat with a tool-capable model.`
      }, retry));
    } catch (error) {
      failLocal(lmStudioName, error, retry);
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const removeLmStudioLocalIntegration = async () => {
    if (localIntegrationAction) return false;
    setLocalIntegrationAction("remove-lm-studio");
    beginLocal(`Unpairing ${lmStudioName}…`);
    try {
      await localClientPackages.removeLmStudio();
      await refreshLmStudioIntegrationStatus();
      reportLocal({ tone: "ready", message: `QC Remote was unpaired from ${lmStudioName}. Other MCP servers were not changed.` });
      return true;
    } catch (error) {
      failLocal(lmStudioName, error, "Unpair");
      return false;
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const openLmStudio = async () => {
    try {
      await localClientPackages.openLmStudio();
      reportLocal({ tone: "ready", message: lmStudioSwitch
        ? `${lmStudioName} opened. In ${lmStudioSwitch}, enable QC Remote, then start a new chat with a tool-capable model.`
        : `${lmStudioName} opened. Start a new chat with a tool-capable model.` });
      await refreshLmStudioIntegrationStatus();
    } catch (error) {
      failLocal(lmStudioName, error, "Open");
    }
  };

  const openLmStudioDownload = async () => {
    const retry = lmStudioRetryLabel();
    try {
      await localClientPackages.openLmStudioDownload();
      // The URL is unchanged: lmstudio.ai is still the publisher's site and
      // still the right place to get the app. What it ships from there is
      // Bionic whatever this machine currently has, so this one sentence names
      // Bionic outright rather than following the row - which is also how an
      // owner of the old build learns that updating renames it.
      reportLocal({ tone: "action-required", message: `The official ${BIONIC_APP_NAME} download page opened. Install it, then press Refresh here.` });
    } catch (error) {
      // The row's own word, because this handler is reached from "Install
      // Bionic" on an empty machine and from "Update LM Studio" on a legacy
      // one. A fixed "Install ..." named a button half of those rows do not
      // render, which is the defect `retryPhrase` exists to prevent.
      failLocal(lmStudioName, error, retry);
    }
  };

  const installJanLocalIntegration = async () => {
    if (localIntegrationAction) return;
    const retry = janRetryLabel();
    setLocalIntegrationAction("jan");
    beginLocal("Pairing Jan…");
    try {
      const result = await localClientPackages.installJan();
      await refreshJanIntegrationStatus();
      reportLocal(localPairOutcome("Jan", {
        installed: result.installed,
        nextStep: "QC Remote is paired with Jan. Open Jan, enable QC Remote in Settings → MCP Servers, then start a chat with a tool-capable model."
      }, retry));
    } catch (error) {
      failLocal("Jan", error, retry);
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const removeJanLocalIntegration = async () => {
    if (localIntegrationAction) return false;
    setLocalIntegrationAction("remove-jan");
    beginLocal("Unpairing Jan…");
    try {
      await localClientPackages.removeJan();
      await refreshJanIntegrationStatus();
      reportLocal({ tone: "ready", message: "QC Remote was unpaired from Jan. Other MCP servers were not changed." });
      return true;
    } catch (error) {
      failLocal("Jan", error, "Unpair");
      return false;
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  const openJan = async () => {
    try {
      await localClientPackages.openJan();
      reportLocal({ tone: "ready", message: "Jan opened. Enable QC Remote in Settings → MCP Servers, then start a chat with a tool-capable model." });
      await refreshJanIntegrationStatus();
    } catch (error) {
      failLocal("Jan", error, "Open");
    }
  };

  const openJanDownload = async () => {
    try {
      await localClientPackages.openJanDownload();
      reportLocal({ tone: "action-required", message: "The official Jan download page opened. Install it, then press Refresh here." });
    } catch (error) {
      failLocal("Jan", error, "Install Jan");
    }
  };

  const copyOtherMcpSetup = async () => {
    if (localIntegrationAction) return;
    setLocalIntegrationAction("other");
    beginLocal("Copying the STDIO connection…");
    try {
      const setup = await localClientPackages.otherMcpStdioSetup();
      await navigator.clipboard.writeText(setup.configuration);
      setOtherMcpStatus(setup);
      reportLocal({ tone: "ready", message: "The standard STDIO connection was copied. Paste it into your desktop app's MCP configuration and enable QC Remote." });
    } catch (error) {
      reportLocal({
        tone: "error",
        message: "QC Remote could not copy the STDIO connection. Try Copy STDIO setup again, then copy the details if it keeps failing.",
        detail: error instanceof Error ? error.message : String(error)
      });
    } finally {
      setLocalIntegrationAction(null);
    }
  };

  useEffect(() => {
    const onApplicationShortcut = (event: globalThis.KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const textEntry = target?.matches("input, textarea, [contenteditable=true]");
      if (event.ctrlKey && event.key.toLowerCase() === "s") {
        event.preventDefault();
        if (event.shiftKey) void openDeviceSave();
        else void saveWorkspace();
        return;
      }
      if (!textEntry && event.ctrlKey && !event.shiftKey && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void undoLastAction();
        return;
      }
      if (!textEntry && event.ctrlKey && (event.key.toLowerCase() === "y" || (event.shiftKey && event.key.toLowerCase() === "z"))) {
        event.preventDefault();
        void redoLastAction();
        return;
      }
      if (!textEntry && event.ctrlKey && event.key.toLowerCase() === "c") {
        event.preventDefault();
        void copySelectedBlockSettings();
        return;
      }
      if (!textEntry && event.ctrlKey && event.key.toLowerCase() === "v") {
        event.preventDefault();
        void pasteSelectedBlockSettings();
        return;
      }
      if (!textEntry && event.key === "F1") {
        event.preventDefault();
        void helpWindows.open("guide").catch(actionFailed);
        return;
      }
      if (event.key === "Escape") {
        // A dialog, menu or editor that handled this Escape has already
        // answered it; only an unhandled Escape closes the open surfaces.
        if (event.defaultPrevented) return;
        if (deviceScreenView) void closeDeviceScreen();
        sceneWorkflow.closeCommand();
        sceneWorkflow.closeLabelColor();
        deviceSettingsWorkflow.close();
        tempoWorkflow.close();
        setPresetDirectoryOpen(false);
        setRoutePicker(undefined);
        closeAddDevicePicker();
      }
    };
    window.addEventListener("keydown", onApplicationShortcut);
    return () => window.removeEventListener("keydown", onApplicationShortcut);
  }, [blockClipboard, closeDeviceScreen, commandPending, connection.demo, deviceScreenView, deviceSettingsWorkflow, openDeviceSave, redoEntry, saveWorkspace, sceneWorkflow, selectedBlockId, snapshot, tempoWorkflow, undoEntry]);

  const menuSelect = (item: MenuCommand) => {
    switch (item) {
      case "settings": void appWindows.open("settings").catch(actionFailed); break;
      case "open-workspace": void openWorkspace(); break;
      case "save-workspace": void saveWorkspace(); break;
      case "save-workspace-as": void saveWorkspace(true); break;
      case "save-preset-to-device": void openDeviceSave(); break;
      case "rename-current-preset": void renameCurrentPreset(); break;
      case "open-preset-directory": void openPresetBrowser(); break;
      case "about": void helpWindows.open("about").catch(actionFailed); break;
      case "device-info": void showDeviceInformation(); break;
      case "add-block": void openAddBlock(); break;
      case "edit-routing": {
        // The device has no menu-driven entry to this screen at all - the
        // unit only ever reaches it by tapping the Grid's own S/M marker or
        // (for a fresh split) its new placement target. The closest real
        // landing for a keyboard/menu path is the first row that already
        // branches; with none, name the two Grid-only ways to reach it
        // rather than opening a dialog the device does not have.
        const splitRow = snapshot.routes.find((route) => route.splitColumn !== undefined)?.row;
        if (splitRow !== undefined) void openRoutingNodeEditor(splitRow, "splitter");
        else setNotice("No signal splits yet - tap the + marker on an even row's signal line to add one, or a row's S/M marker once it has one.");
        break;
      }
      case "user-guide": void helpWindows.open("guide").catch(actionFailed); break;
      case "export-diagnostics": void exportDiagnostics().then(setNotice).catch(actionFailed); break;
      case "report-bug": void helpWindows.open("feedback", "bug").catch(actionFailed); break;
      case "send-feedback": void helpWindows.open("feedback", "feature").catch(actionFailed); break;
      case "view-fit": setSurfaceView("fit"); setNotice("Hardware surface fitted to the application window."); break;
      case "view-actual": setSurfaceView("actual"); setNotice("Hardware surface set to its 96-DPI physical-width approximation."); break;
      case "connect":
      case "reconnect": void connect("reconnect", undefined, true); break;
      case "disconnect": void disconnectDevice(); break;
      case "reset-session": void connect("reset", undefined, true); break;
      case "refresh-state": void refreshSnapshot(); break;
      case "discard-changes": void reloadPreset(); break;
      case "open-tuner": void showDeviceView("tuner"); break;
      case "open-gig-view": void showDeviceView("gig"); break;
      case "open-io-settings": void showIoSettings(); break;
      case "previous-preset": void navigatePreset(-1); break;
      case "next-preset": void navigatePreset(1); break;
      case "set-tempo": tempoWorkflow.openTempo(); break;
      case "select-mode-0": void chooseModeSlot(0); break;
      case "select-mode-1": void chooseModeSlot(1); break;
      case "select-mode-2": void chooseModeSlot(2); break;
      case "export-preset-library": setNotice("Preset-library export will use untouched native .pb files; the raw-file transfer is not enabled yet."); break;
      case "device-backup": void createDeviceBackup(); break;
      case "toggle-fullscreen": void (document.fullscreenElement ? document.exitFullscreen?.() : document.documentElement.requestFullscreen?.()); break;
      case "undo": void undoLastAction(); break;
      case "redo": void redoLastAction(); break;
      case "copy-preset": copyCurrentPreset(); break;
      case "paste-preset": void pasteCurrentPreset(); break;
      case "copy-preset-text": void copyPresetAsText(); break;
      case "paste-preset-text": void pastePresetFromText(); break;
      case "copy-setlist": void copySetlist(); break;
      case "paste-setlist": void pasteSetlist(); break;
      case "copy-block-settings": void copySelectedBlockSettings(); break;
      case "paste-block-settings": void pasteSelectedBlockSettings(); break;
      case "exit": void exitApp(); break;
      default: item satisfies never;
    }
  };

  const handleCorOsContextAction = (action: CorOsContextAction, returnFocusTarget?: "grid-menu") => {
    if (action === "edit-details") void openDeviceSave();
    // The Grid's own Settings row opens the device's real multi-section
    // Settings screen (Account/System/Device/MIDI) in-app - never the
    // separate app-preferences window the MenuBar's own "Settings…" item
    // opens (menuSelect's "settings" case below, untouched).
    else if (action === "settings") deviceSettingsWorkflow.openSettings(returnFocusTarget);
    else if (action === "copy-scene") sceneWorkflow.openCommand(false);
    else if (action === "swap-scene") sceneWorkflow.openCommand(true);
    else if (action === "preset-midi-out") setScreenDestination({ view: "midi-out", returnFocusTarget });
    else if (action === "modes-configuration") { setScreenDestination({ view: "modes", returnFocusTarget }); void modesWorkflow.refresh(); }
    else if (action === "tempo") tempoWorkflow.openTempo(returnFocusTarget);
    else if (action === "cpu-monitor") setScreenDestination({ view: "cpu-monitor", returnFocusTarget });
    else setNotice(corOsUnavailableContextActionMessage(action));
  };

  const disabledContextActions: Partial<Record<CorOsContextAction, string>> = {
    "create-new": "Creating an empty preset is not exposed by the current semantic gateway.",
    favorite: "Favorites require library identity that is not available for the active working buffer.",
    "delete-preset": "Delete the preset from the Preset Directory, where its exact stored position can be verified.",
    "new-capture": "The Neural Capture creation workflow is not exposed by the current semantic gateway."
  };
  // Device Settings and Tempo are device screens: they render in the bezel's
  // own 800x480 canvas, not as a sibling that fills the whole window.
  const screenContent = productionAuditScreen ? <CorOsProductionScreenAudit view={productionAuditScreen} snapshot={snapshot} />
    : deviceSettingsWorkflow.open ? <CorOsDeviceSettings workflow={deviceSettingsWorkflow} onClose={deviceSettingsWorkflow.close} />
    : tempoWorkflow.open ? <CorOsTempoScreen workflow={tempoWorkflow} snapshot={snapshot} queueTempo={queueTempo} onSelectScene={(index) => void chooseScene(index)} onClose={tempoWorkflow.close} />
    : deviceScreenView === "tuner" ? <CorOsTunerScreen workflow={tunerWorkflow} onClose={() => void closeDeviceScreen()} />
    : deviceScreenView === "looper-editor" ? <LooperXScreen workflow={looperWorkflow} activeScene={snapshot.activeScene} onSelectScene={(scene) => void chooseScene(scene)} onParameters={async () => {
      const looper = snapshot.blocks.find((block) => block.modelId === 27_000 || block.name === "Looper X");
      if (!looper) { setNotice("Add a Looper X block to this preset before opening its parameters."); return; }
      await looperWorkflow.run("performMode", 1);
      setDeviceScreenView(null);
      await gridWorkflow.openBlock(looper);
    }} onMenu={async () => {
      const looper = snapshot.blocks.find((block) => block.modelId === 27_000 || block.name === "Looper X");
      if (!looper) { setNotice("Add a Looper X block to this preset before opening its menu."); return; }
      await looperWorkflow.run("performMode", 1);
      setParameterMenuInitiallyOpen(true);
      setDeviceScreenView(null);
      await gridWorkflow.openBlock(looper);
    }} onClose={() => void closeDeviceScreen()} />
    : deviceScreenView === "modes" ? <ModesConfigurationScreen returnFocusTarget={deviceScreenViewState.returnFocusTarget} workflow={modesWorkflow} onClose={() => setDeviceScreenView(null)} />
    : deviceScreenView === "midi-out" ? <PresetMidiOutEditor returnFocusTarget={deviceScreenViewState.returnFocusTarget} workflow={midiOutWorkflow} onClose={() => setDeviceScreenView(null)} />
    : deviceScreenView === "cpu-monitor" ? <CorOsCpuMonitor returnFocusTarget={deviceScreenViewState.returnFocusTarget} snapshot={snapshot} cpuLoad={cpuLoad} cpuColumns={cpuColumns} onClose={() => setDeviceScreenView(null)} />
    : deviceScreenView === "global-eq" ? <CorOsGlobalEqScreen workflow={ioEqWorkflow} onBack={() => setDeviceScreenView("io-overview")} />
    : deviceScreenView?.startsWith("io-") ? <CorOsIoSettingsScreen workflow={ioEqWorkflow} onGlobalEq={() => setDeviceScreenView("global-eq")} onClose={() => void closeDeviceScreen()} />
    : undefined;

  return <div className="app-shell">
    <MenuBar
      menus={appMenus}
      onSelect={menuSelect}
      connection={connection}
      deviceReady={deviceReady}
      syncProgress={syncProgress}
      busy={commandPending || syncProgress !== null || awaitingDetection.current}
      localIntegrationAction={localIntegrationAction}
      controlAccessMode={controlAccessMode}
      onControlAccessModeChange={changeControlAccessMode}
      claudeApprovalLapsed={claudeApprovalLapsed}
      localStatusChecking={localStatusChecking}
      claudeStatusRefreshFailed={claudeStatusRefreshFailed}
      chatgptStatusRefreshFailed={chatgptStatusRefreshFailed}
      antigravityStatusRefreshFailed={antigravityStatusRefreshFailed}
      lmStudioStatusRefreshFailed={lmStudioStatusRefreshFailed}
      janStatusRefreshFailed={janStatusRefreshFailed}
      otherMcpStatusRefreshFailed={otherMcpStatusRefreshFailed}
      runtime={runtime}
      deviceName={snapshot.deviceName}
      physicalDevices={physicalQcDevices}
      selectedDeviceId={selectedPhysicalId}
      onSelectDevice={(id) => void connect("reconnect", id, true)}
      presetLabel={presetLabel}
      events={connectionEvents}
      relayStatus={relayStatus}
      relayPending={relayPending}
      relayPairingStarting={relayPairingStarting}
      relayPairingError={relayPairingError}
      relayPairingQrCode={relayBrowserPairing?.qrCode}
      relayPairingUrl={relayBrowserPairing?.pairUrl}
      onRelayPanelOpen={setRemotePanelOpen}
      relayConnectors={relayConnectors}
      relayConnectorsError={relayConnectorsError}
      relayUnlinkingGrantId={relayUnlinkingGrantId}
      onRelayPairingOpen={() => void openRelayPairUrl()}
      onOpenRelayConnector={() => void openRelayConnector()}
      relayConnectorAddress={`${publicRelayEndpoint}/mcp`}
      onShareRelayConnector={() => void shareRemoteConnectorAddress()}
      onRelayReconnect={() => void reconnectPublicRelay()}
      onRelayPairingRefresh={() => void beginBrowserRelayPairing()}
      onRelayPairingShare={() => void shareRelayPairing()}
      onRelayUnpair={() => void unpairRelayAccount()}
      onRelayDeleteAccount={() => void openRemoteAccountDeletionPage()}
      onRelayUnlinkConnector={(grantId) => void unlinkRelayConnector(grantId)}
      claudeExtensionStatus={claudeExtensionStatus}
      claudeInstallPending={claudeInstallPending}
      chatgptIntegrationStatus={chatgptIntegrationStatus}
      chatgptRestartRequired={chatgptRestartRequired}
      chatgptRemovalRestartRequired={chatgptRemovalRestartRequired}
      antigravityIntegrationStatus={antigravityIntegrationStatus}
      lmStudioIntegrationStatus={lmStudioIntegrationStatus}
      janIntegrationStatus={janIntegrationStatus}
      otherMcpStatus={otherMcpStatus}
      onInstallClaude={() => void installClaudeDesktopExtension()}
      onOpenClaude={() => void openClaudeDesktop()}
      onOpenClaudeExtensions={() => openClaudeExtensionsPage()}
      onOpenClaudeDownload={() => void openClaudeDesktopDownload()}
      onInstallChatgpt={() => void installChatgptLocalIntegration()}
      onRemoveChatgpt={removeChatgptLocalIntegration}
      onOpenChatgpt={() => void openChatgptDesktop()}
      onOpenChatgptDownload={() => void openChatgptDesktopDownload()}
      onOpenChatgptConfiguration={() => void openChatgptConfiguration()}
      onOpenAntigravityDownload={() => void openAntigravityDownload()}
      onInstallAntigravity={() => void installAntigravityLocalIntegration()}
      onRemoveAntigravity={removeAntigravityLocalIntegration}
      onOpenAntigravity={() => void openAntigravity()}
      onInstallLmStudio={() => void installLmStudioLocalIntegration()}
      onRemoveLmStudio={removeLmStudioLocalIntegration}
      onOpenLmStudio={() => void openLmStudio()}
      onOpenLmStudioDownload={() => void openLmStudioDownload()}
      onInstallJan={() => void installJanLocalIntegration()}
      onRemoveJan={removeJanLocalIntegration}
      onOpenJan={() => void openJan()}
      onOpenJanDownload={() => void openJanDownload()}
      onCopyOtherMcpSetup={() => void copyOtherMcpSetup()}
      onRefreshLocalIntegrations={() => void refreshLocalIntegrations(true)}
      onLocalPanelChange={setLocalPanelOpen}
      onConnect={() => void connect("reconnect", undefined, true)}
      onDisconnect={() => void disconnectDevice()}
      demoPending={demoPending}
      onStartDemo={() => void startDemoDevice()}
      onStopDemo={stopDemoDevice}
      deviceHeldInBackground={deviceHeldInBackground}
      onResumeDevice={() => void resumeDeviceSession()}
      onClearEvents={() => setConnectionEvents([])}
      onOpenChange={setAppMenuOpen}
    />

    <div className="app-content">
      <main className={`workspace view-${surfaceView}`}>
        <QuadCortexSurface formFactor={formFactor} snapshot={snapshot} selectedBlockId={selectedBlockId} skin={skin} screenView={fixtureScreenView ?? deviceScreenView ?? undefined} screenContent={screenContent} deviceInformation={deviceInformation} onCloseScreen={() => void closeDeviceScreen()} onDeviceScreenGesture={(x, y, toX, toY, kind) => void forwardDeviceScreenGesture(x, y, toX, toY, kind)} onOpenGigView={() => void showDeviceView("gig")} onOpenIoView={() => void showIoSettings()} onEditScene={sceneWorkflow.openLabelColor} onRecallGigPreset={(entry) => void presetWorkflow.recall(entry)} onCopyGigScene={(scene) => sceneWorkflow.openCommandForScene(scene, false)} onSwapGigScene={(scene) => sceneWorkflow.openCommandForScene(scene, true)} sceneEditor={sceneWorkflow.labelColorOpen ? { sourceScene: sceneWorkflow.sourceScene, setSourceScene: sceneWorkflow.setSourceScene, scenes: snapshot.scenes, label: sceneWorkflow.label, setLabel: sceneWorkflow.setLabel, color: sceneWorkflow.color, setColor: sceneWorkflow.setColor, colors: sceneWorkflow.colors, pending: commandPending, saveLabel: () => void sceneWorkflow.saveLabel(), saveColor: () => void sceneWorkflow.saveColor(), onClose: sceneWorkflow.closeLabelColor } : undefined} onAction={handleHardwareAction} onMasterVolumeInteractionStart={continuousControls.beginMasterVolumeInteraction} onMasterVolumeInteractionEnd={continuousControls.endMasterVolumeInteraction} onOpenPreset={() => void openPresetBrowser()} onUndo={() => void undoLastAction()} canUndo={Boolean(undoEntry) || deviceUndoAvailable} undoLabel={undoEntry?.label} onSave={openDeviceSave} onOpenRouting={openRoutePicker} onRefresh={() => void refreshSnapshot()} savePreset={presetWorkflow.saveProps} presetDirectory={presetWorkflow.directoryProps} gigPresetList={presetWorkflow.presetList} routingPicker={routingWorkflow.pickerProps} addDevicePicker={gridWorkflow.addPickerProps} expressionAssignment={expressionAssignment} laneControlEditor={laneControlEditor} parameterEditor={parameterEditorBindings} onContextAction={handleCorOsContextAction} disabledContextActions={disabledContextActions} />
      </main>
    </div>

    <div className="status-strip" role="status">
      <span className="status-symbol">i</span>
      <span className="status-notice">{visibleNotice}</span>
      <span className="status-context"><span className={`context-pill phase-${liveIndicator.phase}${liveIndicator.backgroundSync ? " is-background-sync" : ""}`} title={liveIndicator.backgroundSync ? "Live and synchronized · still receiving library data from the Quad Cortex" : undefined}>{liveIndicator.label}</span>{awaitingDeviceData && <span className="context-pill is-awaiting-data" role="status">WAITING FOR DEVICE DATA</span>}{snapshot.gridLayoutComplete === false && <span className="context-pill is-awaiting-data" role="status">{gridRefreshPending ? "REFRESHING GRID…" : "GRID INCOMPLETE"}</span>}<strong>{snapshot.presetLocation} · {snapshot.presetName}</strong><span>Scene {sceneLetter(snapshot.activeScene)}</span>{snapshot.dirty && <span className="dirty-state">UNSAVED</span>}{workspaceName && <span>{workspaceName}</span>}<span>{selectedBlockId ? snapshot.blocks.find((block) => block.id === selectedBlockId)?.name : "No selection"}</span></span>
    </div>


    {/* Copy/swap is still the Grid context-menu command. Scene label/color is
        rendered by QuadCortexSurface as the current device screen, reached
        through Scene mode -> Gig View -> the scene tile's Edit control. */}
    {sceneWorkflow.command && <CorOsSceneCommand scenes={snapshot.scenes} sourceScene={sceneWorkflow.sourceScene} swap={sceneWorkflow.command === "swap"} pending={commandPending} onSelect={(destination) => void sceneWorkflow.chooseDestination(destination)} onCancel={sceneWorkflow.closeCommand} returnFocusTarget={sceneWorkflow.commandReturnTarget} />}
    {textPromptOverlay}
  </div>;
}
