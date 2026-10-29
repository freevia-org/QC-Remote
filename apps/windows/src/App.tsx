import { formatNativeBackupSaveNotice } from "@qc-remote/ui";
import { applicationModifier, shortcutLabel } from "./host-platform.ts";
import { exposeDesktopDemo, observeDemoBrokerInstance } from "./demo-broker.ts";
import { demoRegistrationNeverAttempted, observeDemoSelectionHold } from "./demo-broker-lifecycle.ts";
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json";
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { type ConnectionState, type DeviceActionResult, type DiagnosticsReport, type PresetSnapshot, type RuntimeStatus, type WorkspaceDocument } from "@qc-remote/client";
import { modeSlotIsActive, demoBlockDetails, demoSnapshot, sceneLetter, SupersededPresetOperation, saveWorkspaceDocument, type ControlAccessMode, type PublicRelayConnector } from "@qc-remote/core";
import { formFactors, skins } from "@qc-remote/form-factors";
import { QC_BRAND } from "@qc-remote/theme";
import { CONTROL_ACCESS_OPTIONS, CONNECTING_PLACEHOLDER_SNAPSHOT, connectingPlaceholderSnapshot, corosFixtureConfiguration, createGridRepairCoordinator, refreshGridSnapshotOnce, createQcDeviceDisplay, QcDeviceDisplay, QcDeviceDisplayOverlay, createDemoDeviceSession, IDLE_PLACEHOLDER_SNAPSHOT, usePresetMidiOutWorkflow, qcLaneControlPanelBindings, qcLiveIndicator, qcParameterEditorBindings, readControlAccessMode, useBlockEditorSession, useContinuousControlWorkflow, useDeviceSettingsWorkflow, useExpressionAssignment, useIoEqWorkflows, useLooperXWorkflow, useModesConfigurationWorkflow, useNativeStateRecovery, usePublicRelayWorkflow, useQcConnectionWorkflow, useQcController, useQcCpuTelemetry, useQcDeviceUndo, useQcDeviceViews, useQcScreenFollow, useQcFollowSetting, consumeQcDisplayStates, useQcScreenDestination, useQcLiveState, useQcSurfaceActions, useQcWorkflows, useTempoWorkflow, useTunerWorkflow, useWriteHealth, writeControlAccessMode, capturePresetText, captureSetlistForDemo, importCapturedSetlistIntoDemo, isGridDevice, isIncompleteGridRefreshFailure, parsePresetText, pastePresetIntoDemo, presetReplayConfirmation, presetReplaySummary, refreshGridSnapshot, replayPresetOnDevice, virtualDeviceStoreOf, type CapturedSetlist } from "@qc-remote/ui";
import { CorOsProductionScreenAudit } from "@qc-remote/ui/dev";
import { appWindows, brokerDevice, DEVICE_INFO_EVENTS, diagnosticsFiles, externalLinks, helpWindows, listenTauri, localAccess, localClientPackages, macosMenuEventsReady, physicalDevices, type PhysicalQcDevice, onDevicePhase, publicRelay, publishWindowState, serveAppWindowRequests, serveDiagnosticsRequests, SETTINGS_EVENTS, tauriTransport, virtualDeviceStorage, WORKSPACE_EVENTS, workspaceFiles, type AntigravityLocalIntegrationStatus, type ChatGptLocalIntegrationStatus, type ClaudeDesktopExtensionStatus, type JanLocalIntegrationStatus, type LmStudioLocalIntegrationStatus, type LocalAccessStatus, type OtherMcpStdioSetup } from "./tauri-transport";
import { createWindowsQcTransport } from "./qc-transport";
import { useWindowsDeviceFrames } from "./use-windows-device-frames";
import { useWindowsPrompts } from "./use-windows-prompts";
import { BROWSER_PAIRING_EXPIRED, beginBrowserPairingOnPanelOpen, browserPairingPollFailureMessage, createBrowserPairingSession, startBrowserPairingLifecycle } from "./browser-pairing-session";
import { shouldCloseBlockEditorOnEscape } from "./block-editor-keyboard";
import { saveOpenedWorkspaceCopy } from "./workspace-copy";
import { captureGatewaySession } from "@qc-remote/ui";
import { divider, MenuBar, type AppMenu, type ConnectionEvent, type MenuCommand, type MenuItem } from "./menu-bar";
import { antigravityIntegrationPresentation, BIONIC_APP_NAME, chatgptIntegrationPresentation, claudeExtensionPresentation, lmStudioAppName, lmStudioEnableSwitch, localAccessNotice, localActionFailure, localPairOutcome, managedMcpIntegrationPresentation, type LocalActionOutcome } from "./local-integration-presentation";
import { brokerPhaseDetectsDevice, isQcConnectionFailure, qcConnectionDuringBrokerPhase, qcAutomaticConnectHeld, qcConnectionIsVerified, QC_BROKER_TRUTH_UNKNOWN, QC_CONNECT_STALL_MS, QC_FOLLOW_DEMO_ATTEMPTS, QC_FOLLOW_INITIAL, qcBackgroundDeviceLive, qcDisconnectOutcome, qcFollowAfterUserConnect, qcFollowBroker, qcInventoryDeviceReady, qcInventoryWhenUnreachable, qcLatchUserDisconnect, qcObserveBroker, qcPhaseListenerMayFollow, qcRecordLiveSessionLoss, qcSameBrokerTruth, qcStopDemoOutcome, qcUserDisconnectAfter, qcUserDisconnectHolds, type QcBrokerObservation, type QcBrokerTruth, type QcFollowState, type QcUserDisconnect, qcShouldDisconnectForDemo, qcShouldFollowDevicePhase, qcVirtualSessionIsReady, qcVisibleStatusNotice, qcWatchConnectStall, runtimeHasSynchronizedQc, waitForSynchronizedQc, qcReconcilePhysicalDevices, type QcConnectStall, syncProgressAt, syncProgressFinishMs } from "./qc-readiness";
import { DEFAULT_BROKER_TRAY_STATUS, summarizeUpdaterRelay, type BrokerTrayStatus, type DeviceInfoState, type SettingsState, type WorkspaceState } from "./app-window-content";
import { brokerTray, changeBrokerTray } from "./tauri-transport";
import { callTauri } from "./tauri-invoke.ts";
import appPackage from "../package.json";
import QRCode from "qrcode";
import { qcLiveSessionCanObserveState, qcStreamRestartClearsBulkOperation } from "@qc-remote/ui";
import { qcDisconnectedConnection } from "./qc-readiness";

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
  const macMenuConnectRef = useRef<(() => void) | null>(null);
  const macMenuDisconnectRef = useRef<(() => void) | null>(null);
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
  const qcController = useQcController(corpusFixtureEnabled ? fixtureInitialSnapshot : IDLE_PLACEHOLDER_SNAPSHOT);
  const {
    snapshot, snapshotRef, setSnapshot,
    resetCommands, reconcileFrame
  } = qcController;
  useEffect(() => { if (corpusFixtureEnabled) setSnapshot(fixtureInitialSnapshot); }, [setSnapshot]);
  const [selectedBlockId, setSelectedBlockId] = useState("");
  const [formFactorId, setFormFactorId] = useState("quad-cortex-large");
  const [notice, setNotice] = useState("Disconnected. Select Connect to use a Quad Cortex, or explicitly select Demo.");
  const [demoRestartNotice, setDemoRestartNotice] = useState<string | null>(null);
  const [gridRefreshPending, setGridRefreshPending] = useState(false);
  const { observe: observeWriteHealth } = useWriteHealth({
    enabled: Boolean(window.__TAURI_INTERNALS__) && !corpusFixtureEnabled,
    recoveryInstruction: "the session is reset",
    onNotice: setNotice
  });
  const [connectionPending, setConnectionPending] = useState(false);
  const mutationBusy = useSyncExternalStore(qcController.lifetime.mutations.subscribe, qcController.lifetime.mutations.snapshot);
  const commandPending = connectionPending || mutationBusy;
  // Exclusive workflow ownership is derived from tokens, never captured booleans.
  const setWorkflowPending = useCallback((_pending: boolean | ((current: boolean) => boolean)) => {}, []);
  const appOperationMounted = useRef(true);
  const appOperationEpoch = useRef(0);
  useEffect(() => {
    appOperationMounted.current = true;
    return () => { appOperationMounted.current = false; appOperationEpoch.current++; };
  }, []);
  const { prompts: appPrompts, overlay: textPromptOverlay, active: appPromptActive } = useWindowsPrompts();
  const [controlAccessMode, setControlAccessMode] = useState<ControlAccessMode>(storedAccessMode);
  const { follow: followQcScreen, setFollow: setFollowQcScreen } = useQcFollowSetting(localStorage);
  const [relayBrowserPairing, setRelayBrowserPairing] = useState<{ pairUrl: string; expiresAt: number; qrCode: string }>();
  const [relayPairingStarting, setRelayPairingStarting] = useState(false);
  const [relayPairingError, setRelayPairingError] = useState<string>();
  const [relayWithdrawalPending, setRelayWithdrawalPending] = useState(false);
  const [relayConnectors, setRelayConnectors] = useState<PublicRelayConnector[]>([]);
  const [relayConnectorsError, setRelayConnectorsError] = useState<string>();
  const [relayUnlinkingGrantId, setRelayUnlinkingGrantId] = useState<string>();
  const relayConnectorRead = useRef(0);
  const relayConnectorMounted = useRef(true);
  const relayConnectorMountEpoch = useRef(0);
  const relayConnectorMutation = useRef<{ grantId?: string } | undefined>(undefined);
  useEffect(() => {
    relayConnectorMounted.current = true;
    setRelayUnlinkingGrantId(relayConnectorMutation.current?.grantId);
    return () => {
      relayConnectorMounted.current = false;
      relayConnectorMountEpoch.current += 1;
      relayConnectorRead.current += 1;
    };
  }, []);
  // Only an unpaired host starts pairing on REMOTE open. A paired host's
  // status panel remains idle until an explicit new-pairing action.
  const relayPanelOpen = useRef(false);
  const relayPairingRestartRequested = useRef(false);
  const relayPairingSession = useMemo(() => createBrowserPairingSession({ begin: () => publicRelay.browserPairing!.begin(), cancel: () => publicRelay.browserPairing!.cancel!() }), []);
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
  const localStatusMounted = useRef(true);
  const localStatusEpoch = useRef(0);
  const localStatusReads = useRef(new Map<string, number>());
  useEffect(() => {
    localStatusMounted.current = true;
    return () => {
      localStatusMounted.current = false;
      localStatusEpoch.current++;
      localStatusRefreshInFlight.current = false;
    };
  }, []);
  const captureLocalStatusRead = useCallback((provider: string) => {
    const epoch = localStatusEpoch.current;
    const read = (localStatusReads.current.get(provider) ?? 0) + 1;
    localStatusReads.current.set(provider, read);
    return () => localStatusMounted.current && epoch === localStatusEpoch.current && read === localStatusReads.current.get(provider);
  }, []);
  const editor = useBlockEditorSession();
  const { details: blockDetails } = editor;
  const [parameterMenuInitiallyOpen, setParameterMenuInitiallyOpen] = useState(false);
  const [workspacePath, setWorkspacePath] = useState<string>();
  const [workspaceName, setWorkspaceName] = useState<string>();
  const [loadedWorkspace, setLoadedWorkspace] = useState<WorkspaceDocument>();
  const workspaceDocumentIdentity = useRef<{ document: WorkspaceDocument | undefined; id: string } | undefined>(undefined);
  if (!workspaceDocumentIdentity.current || workspaceDocumentIdentity.current.document !== loadedWorkspace) {
    workspaceDocumentIdentity.current = { document: loadedWorkspace, id: globalThis.crypto.randomUUID() };
  }
  const [surfaceView, setSurfaceView] = useState<"fit" | "actual">("fit");
  const gridRefreshRequest = useRef(0);
  const gridRepair = useRef(createGridRepairCoordinator());
  const screenDestination = useQcScreenDestination();
  const deviceScreenView = screenDestination.view;
  const setDeviceScreenView = screenDestination.setView;
  const cpuTelemetry = useQcCpuTelemetry(qcController.lifetime);
  const consumeLiveState = useQcLiveState({
    reconcileFrame,
    editor,
    onStates: (states, frame) => {
      consumeQcDisplayStates(states, { setView: setDeviceScreenView, cpu: cpuTelemetry, undo: deviceUndo, follow: followQcScreen });
      for (const state of states) {
        if (state.kind === "gridTopology" && state.gridLayoutComplete === false
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
  const [setlistClipboard, setSetlistClipboard] = useState<{ serial: string; source: "device" | "virtual"; setlistKey: string; setlistName: string; captured?: CapturedSetlist }>();
  const [fullScreen, setFullScreen] = useState(Boolean(document.fullscreenElement));
  const [appMenuOpen, setAppMenuOpen] = useState(false);
  const [connectionEvents, setConnectionEvents] = useState<ConnectionEvent[]>([{ at: new Date().toISOString(), event: "app-start", result: "info", detail: `${QC_BRAND.appName} started; waiting for the desktop runtime.` }]);
  const autoConnectStarted = useRef(false);
  const liveSyncFailures = useRef(0);
  const [physicalQcDevices, setPhysicalQcDevices] = useState<PhysicalQcDevice[]>([]);
  const [selectedPhysicalId, setSelectedPhysicalId] = useState<string>();
  const selectedPhysicalRef = useRef<string | undefined>(undefined);
  // Assigned every render below, beside the phase listener it backs up, so
  // this effect's one closure always decides against current state.
  const followReadyInventory = useRef<(observation: QcBrokerObservation) => void>(() => undefined);
  // Broker truth as the inventory poll last saw it (qcObserveBroker): the ref
  // for decisions, the state for what the USB tabs draw.
  const brokerTruth = useRef<QcBrokerTruth>(QC_BROKER_TRUTH_UNKNOWN);
  const [brokerTruthView, setBrokerTruthView] = useState<QcBrokerTruth>(QC_BROKER_TRUTH_UNKNOWN);
  // The automatic follow's backoff and live measurement (qcFollowBroker,
  // performance.now(), which never jumps back). The user's Connect restarts it.
  const followState = useRef<QcFollowState>(QC_FOLLOW_INITIAL);
  // Whether this window already said it stopped taking over the demo (qcFollowBroker's demoGaveUp).
  const followDemoGaveUpNoticed = useRef(false);
  // From the user's Disconnect, failed or not, until their own Connect or a
  // new device session the broker reports (an answered not-ready, then ready,
  // in the same broker generation); a new generation alone never lifts it
  // (qcUserDisconnectAfter).
  const userDisconnected = useRef<QcUserDisconnect | null>(null);
  // Provider loss retains this hold within its ACK-bound broker instance.
  // The owner's restart policy ends Demo on a positively different instance.
  const explicitDemoHeld = useRef(false);
  const demoBrokerInstance = useRef<string | undefined>(undefined);
  const demoSelectionObservationEpoch = useRef(0);
  const demoSelectionChanges = useRef(0);
  // Only refs, so any render's copy (beginLiveRecovery keeps its first) is current.
  const recordLiveSessionLoss = (userAction = false) => {
    followState.current = qcRecordLiveSessionLoss(followState.current, userAction);
  };
  // The connect watchdog's running observation (qcWatchConnectStall), the
  // connect() it watches (numbered per attempt, with where it started from),
  // and the device.reconnect/resetSession requests still unsettled, the ones
  // it gave up on included (qcAutomaticConnectHeld).
  const connectStall = useRef<QcConnectStall | null>(null);
  const connectSerial = useRef(0);
  const waitingConnect = useRef<{ attempt: number; fromDemo: boolean; current: () => boolean } | null>(null);
  const reconnectsOutstanding = useRef(0);
  // Set by the first inventory poll that answers: from then on only the poll
  // decides automatic follows (qcPhaseListenerMayFollow).
  const inventoryPollAnswered = useRef(false);
  useEffect(() => {
    if (!window.__TAURI_INTERNALS__) return;
    let disposed = false;
    let timer: number | undefined;
    const refresh = async () => {
      const observedSelectionEpoch = demoSelectionObservationEpoch.current;
      try {
        const inventory = await physicalDevices.list();
        if (disposed || observedSelectionEpoch !== demoSelectionObservationEpoch.current) return;
        const demoEnded = await observeDemoBrokerEpoch(inventory.brokerInstanceId, observedSelectionEpoch);
        const continuationEpoch = demoEnded ? demoEnded.selectionEpoch : observedSelectionEpoch;
        if (disposed || continuationEpoch !== demoSelectionObservationEpoch.current) return;
        observeDemoBrokerInstance(inventory.brokerInstanceId);
        observeHeldDemoSelection(inventory.demoSelectionHeld, inventory.brokerInstanceId, continuationEpoch);
        setPhysicalQcDevices((previous) => qcReconcilePhysicalDevices(inventory.devices, selectedPhysicalRef.current, previous));
        if (!selectedPhysicalRef.current && inventory.selectedDeviceId) {
          selectedPhysicalRef.current = inventory.selectedDeviceId;
          setSelectedPhysicalId(inventory.selectedDeviceId);
        }
        inventoryPollAnswered.current = true;
        followReadyInventory.current({ kind: "answered", inventoryReady: qcInventoryDeviceReady(inventory.devices, selectedPhysicalRef.current), instanceId: inventory.brokerInstanceId });
      } catch {
        if (disposed) return;
        observeDemoBrokerInstance(undefined);
        // Not the last answer: the broker quit, was released or cannot be
        // reached, so nothing it said earlier may stay green.
        setPhysicalQcDevices((previous) => qcInventoryWhenUnreachable(previous));
        followReadyInventory.current({ kind: "unreachable" });
      }
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
  const demoBrokerCleanup = useRef<(() => Promise<void>) | undefined>(undefined);
  const demoBrokerTransport = useRef<typeof demoDeviceSession.virtual>(undefined);
  const demoBrokerReady = useRef(false);
  const demoBrokerRetirement = useRef<Promise<void>>(Promise.resolve());
  useEffect(() => () => { void demoBrokerCleanup.current?.().catch(() => undefined); }, []);
  const activeGateway = demoDeviceSession.gateway;
  const captureDeviceOperation = () => {
    const attempt = connectionAttempts.capture();
    const origin = qcController.lifetime.checkpoint();
    const epoch = appOperationEpoch.current;
    return captureGatewaySession(activeGateway, () => appOperationMounted.current && epoch === appOperationEpoch.current && attempt.current() && qcController.lifetime.currentSession(origin));
  };
  const [demoPending, setDemoPending] = useState(false);
  // Demo is always an explicit choice; physical availability cannot take it over.
  const demoCanYieldToDevice = useRef(false);
  // A phase can arrive while device.reconnect still owns the gateway. Keep
  // that edge rather than dropping it; if this attempt falls back to Demo,
  // the freshly opened fallback immediately follows the detected device.
  const reconnectAfterCurrentAttempt = useRef(false);
  // The demo open that owns demoPending. A connect() can supersede an open
  // (connectionAttempts), and the superseded open must still clear the flag it
  // set, or the Demo device tab stays "Opening…" and disabled for good.
  const demoOpenToken = useRef(0);
  const demoReconnectTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => { if (demoReconnectTimer.current !== undefined) window.clearTimeout(demoReconnectTimer.current); }, []);
  // Set by startDemoDevice when it leaves an already-ready Quad Cortex's
  // session open rather than closing it, so a person can switch back without
  // a full reconnect (resumeDeviceSession). deviceReady cannot carry this by
  // itself: it requires connection.source === "device", which is false for
  // the whole time the demo is the active routing target.
  const [deviceHeldInBackground, setDeviceHeldInBackground] = useState(false);
  // What the USB tab and Stop demo may claim: held, and still live per broker.
  const backgroundDeviceLive = qcBackgroundDeviceLive(deviceHeldInBackground, connection.source === "virtual", brokerTruthView, runtime);
  const qcTransport = useMemo(() => createWindowsQcTransport(activeGateway, () => snapshotRef.current), [activeGateway]);
  connectionRef.current = connection;
  // beginLiveRecovery is memoized far above showDisconnected's own
  // definition (and must stay stable - see its effect dependents), so it
  // cannot close over showDisconnected directly without freezing on the
  // first render's copy, exactly the staleness connectOnDetection's ref
  // avoids for connect() below. Read through a ref for the same reason.
  const showDisconnectedRef = useRef<(reason: string) => void>(() => undefined);
  const beginLiveRecovery = useCallback((cause: string) => {
    const current = connectionRef.current;
    if (current.source !== "device" || current.demo || current.phase !== "ready") return;
    nativeStateAvailable.current = false;
    // For the automatic follow, decided by the inventory poll (and by a phase
    // push only before any poll answered), which follows the broker again for
    // as long as it reads the device ready (qcFollowBroker, with backoff).
    // The loss also restarts the live measurement, so a drop another path
    // heals before any poll sees it is not counted as live time.
    recordLiveSessionLoss();
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "live-health-failed", result: "failure", detail: cause }]);
    // Loss revokes hardware readiness, never selects a simulated target.
    // The existing inventory follow can resume the broker's next live session.
    showDisconnectedRef.current(`The live connection was lost: ${cause}`);
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
  const deviceCommandSessionKey = `${demoDeviceSession.source}:${snapshot.stateVersion?.session ?? ""}:${sessionReady}:${nativeStateStream.current ?? ""}`;
  const workflows = useQcWorkflows({
    commandSessionKey: deviceCommandSessionKey,
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
    setPending: setWorkflowPending,
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
      // Scene copy/swap opens the shared scene command from the Grid context menu.
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
  const deviceWorkflowSession = {
    lifetime: qcController.lifetime,
    commandSessionKey: deviceCommandSessionKey,
    sessionKey: `${demoDeviceSession.source}:${snapshot.stateVersion?.session ?? ""}:${sessionReady}:${nativeStateStream.current ?? ""}`,
    captureSessionCurrent: () => {
      const attempt = connectionAttempts.capture();
      const source = demoDeviceSession.source;
      const stream = nativeStateStream.current;
      return () => attempt.current() && demoDeviceSession.source === source && nativeStateStream.current === stream;
    }
  };
  const deviceUndo = useQcDeviceUndo({ ...deviceWorkflowSession,
    history: deviceHistory, snapshot, gateway: activeGateway, presetLifetime: qcController.lifetime,
    connected: sessionReady, pending: commandPending, setPending: setWorkflowPending,
    notice: setNotice, fail: actionFailed, reconcile: reconcileWorkflowSnapshot
  });
  const { undoAvailable: deviceUndoAvailable, redoAvailable: deviceRedoAvailable, undo: undoLastAction, redo: redoLastAction } = deviceUndo;
  const demoSession = connection.demo || connection.source === "virtual";
  const expressionWorkflow = useExpressionAssignment({
    commandSessionKey: deviceWorkflowSession.commandSessionKey,
    lifetime: qcController.lifetime,
    gateway: activeGateway,
    sessionKey: `${demoDeviceSession.source}:${snapshot.stateVersion?.session ?? ""}:${sessionReady}:${nativeStateStream.current ?? ""}`,
    captureSessionCurrent: () => {
      const attempt = connectionAttempts.capture();
      const source = demoDeviceSession.source;
      const stream = nativeStateStream.current;
      return () => attempt.current() && demoDeviceSession.source === source && nativeStateStream.current === stream;
    },
    snapshot,
    connected: sessionReady,
    pending: commandPending,
    setPending: setWorkflowPending,
    reconcile: reconcileWorkflowSnapshot,
    recordHistory: recordUndo,
    notice: setNotice,
    fail: actionFailed
  });
  const deviceSettingsWorkflow = useDeviceSettingsWorkflow({
    lifetime: qcController.lifetime,
    ioSettingsError: snapshot.ioSettingsError,
    commandSessionKey: deviceWorkflowSession.commandSessionKey,
    gateway: activeGateway,
    sessionKey: `${connection.source}:${snapshot.stateVersion?.session ?? ""}:${deviceReady}:${nativeStateStream.current ?? ""}`,
    captureSessionCurrent: () => {
      const attempt = connectionAttempts.capture();
      const source = demoDeviceSession.source;
      const stream = nativeStateStream.current;
      return () => attempt.current() && demoDeviceSession.source === source && nativeStateStream.current === stream;
    },
    connected: sessionReady,
    pending: commandPending,
    setPending: setWorkflowPending,
    notice: setNotice,
    fail: actionFailed
  });
  const tempoWorkflow = useTempoWorkflow({ ...deviceWorkflowSession,
    gateway: activeGateway,
    lifetime: qcController.lifetime,
    connected: sessionReady,
    pending: commandPending,
    setPending: setWorkflowPending,
    notice: setNotice,
    fail: actionFailed
  });
  const tunerWorkflow = useTunerWorkflow({ ...deviceWorkflowSession, gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setWorkflowPending, notice: setNotice, fail: actionFailed });
  const looperWorkflow = useLooperXWorkflow({ ...deviceWorkflowSession, gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setWorkflowPending, notice: setNotice, fail: actionFailed });
  const midiOutWorkflow = usePresetMidiOutWorkflow({ lifetime: qcController.lifetime, gateway: activeGateway, snapshot, demo: connection.demo || connection.source === "virtual", onSaved: reconcileWorkflowSnapshot,
    captureSessionCurrent: () => {
      const attempt = connectionAttempts.capture();
      const source = demoDeviceSession.source;
      const stream = nativeStateStream.current;
      return () => attempt.current() && demoDeviceSession.source === source && nativeStateStream.current === stream;
    }
  });
  const modesWorkflow = useModesConfigurationWorkflow({ ...deviceWorkflowSession, gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setWorkflowPending, notice: setNotice, fail: actionFailed });
  const ioEqWorkflow = useIoEqWorkflows({ ...deviceWorkflowSession, gateway: activeGateway, connected: sessionReady, pending: commandPending, setPending: setWorkflowPending, notice: setNotice, fail: actionFailed, snapshot });
  useQcScreenFollow({ ...deviceWorkflowSession,
    gateway: activeGateway, follow: followQcScreen, available: !connection.demo && !corpusFixtureEnabled, ready: deviceReady, destination: screenDestination, notice: setNotice,
    workflows: { grid: gridWorkflow, preset: presetWorkflow, deviceSettings: deviceSettingsWorkflow, tempo: tempoWorkflow, tuner: tunerWorkflow, ioEq: ioEqWorkflow, modes: modesWorkflow, routing: routingWorkflow, scene: sceneWorkflow }
  });
  const continuousControls = useContinuousControlWorkflow({
    controller: qcController,
    gateway: activeGateway,
    connected: sessionReady,
    demo: corpusFixtureEnabled,
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
      const current = looperWorkflow.captureContinuation();
      const destinationCurrent = screenDestination.captureCurrent();
      if (!current() || !destinationCurrent()) return;
      const accepted = await looperWorkflow.run("open");
      if (accepted && current() && destinationCurrent()) setDeviceScreenView("looper-editor");
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
    editorContextKey: JSON.stringify([connection.source, qcController.lifetime.checkpoint().session, qcController.lifetime.version, snapshot.presetIdentity, selectedBlockId, editor.details?.modelId, snapshot.activeScene]),
    referenceOnly: corpusFixtureEnabled,
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
  const statusNotice = qcVisibleStatusNotice(notice, deviceReady, presetLabel);
  const visibleNotice = demoRestartNotice && demoRestartNotice !== statusNotice
    ? `${demoRestartNotice} ${statusNotice}` : statusNotice;
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
        { id: "save-workspace", label: "Save Preset", shortcut: shortcutLabel("Ctrl+S") },
        { id: "save-workspace-as", label: "Save Preset As…" },
        { id: "open-preset-directory", label: "Preset Library…", disabled: !ready },
        { id: "save-preset-to-device", label: "Save to QC…", shortcut: shortcutLabel("Ctrl+Shift+S"), disabled: !ready },
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
        { id: "undo", label: undoEntry ? `Undo ${undoEntry.label}` : "Undo", shortcut: shortcutLabel("Ctrl+Z"), disabled: (!undoEntry && !deviceUndoAvailable) || !ready },
        { id: "redo", label: redoEntry ? `Redo ${redoEntry.label}` : "Redo", shortcut: shortcutLabel("Ctrl+Y"), disabled: (!redoEntry && !deviceRedoAvailable) || !ready },
        divider(),
        { id: "copy-preset", label: "Copy Preset", disabled: !canCopyPreset },
        { id: "paste-preset", label: "Paste Preset", disabled: !canPastePreset },
        { id: "copy-preset-text", label: "Copy Preset as Text", disabled: !ready || !snapshot.presetIdentity },
        { id: "paste-preset-text", label: "Paste Preset from Text", disabled: !ready || !snapshot.presetIdentity },
        { id: "copy-setlist", label: "Copy Setlist", disabled: !ready || !snapshot.setlistKey },
        { id: "paste-setlist", label: "Paste Setlist", disabled: !ready || !setlistClipboard },
        divider(),
        { id: "copy-block-settings", label: "Copy Device", shortcut: shortcutLabel("Ctrl+C"), disabled: !hasSelectedBlock },
        { id: "paste-block-settings", label: "Paste Device", shortcut: shortcutLabel("Ctrl+V"), disabled: !ready || !clipboardCompatible },
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
        ...(snapshot.modeSlots ?? []).map((entry): MenuItem => ({ id: `select-mode-${entry.slot}`, label: entry.label, checked: modeSlotIsActive(snapshot, entry.slot), disabled: !ready })),
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
  }, [blockClipboard, commandPending, deviceReady, deviceRedoAvailable, deviceUndoAvailable, fullScreen, presetClipboard, redoEntry, selectedBlockId, sessionReady, setlistClipboard, snapshot.blocks, snapshot.dirty, snapshot.footswitchModes, snapshot.mode, snapshot.modeSlots, snapshot.presetIdentity, snapshot.presetName, snapshot.presetPosition, snapshot.setlistKey, surfaceView, undoEntry]);

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
    setRelayBrowserPairing(undefined);
    // begin advances the generation synchronously. Catch must observe that
    // new intent, not the previous offer's checkpoint.
    const pending = relayPairingSession.begin();
    const current = relayPairingSession.checkpoint();
    try {
      const operation = await pending;
      if (!operation.current() || !relayPanelOpen.current) return;
      const { offer } = operation;
      if (!offer.pairUrl) throw new Error("The desktop relay did not return a pairing URL.");
      const qrCode = await QRCode.toDataURL(offer.pairUrl, { width: 224, margin: 1, errorCorrectionLevel: "M" });
      if (!operation.current() || !relayPanelOpen.current) return;
      setRelayBrowserPairing({ pairUrl: offer.pairUrl, expiresAt: offer.expiresAt, qrCode });
    } catch (error) {
      if (!current() || !relayPanelOpen.current) return;
      setRelayPairingError(error instanceof Error ? error.message : "QC Remote could not create a pairing code.");
      actionFailed(error);
    } finally {
      relayPairingStartInFlight.current = false;
      setRelayPairingStarting(false);
      if (relayPairingRestartRequested.current && relayPanelOpen.current) {
        relayPairingRestartRequested.current = false;
        void beginBrowserRelayPairing();
      }
    }
  }, [actionFailed, relayPairingSession]);

  const refreshRelayConnectors = useCallback(async () => {
    if (!publicRelay.connectors || !relayConnectorMounted.current || relayConnectorMutation.current) return;
    const read = ++relayConnectorRead.current;
    const epoch = relayConnectorMountEpoch.current;
    const current = () => relayConnectorMounted.current && epoch === relayConnectorMountEpoch.current && read === relayConnectorRead.current;
    try {
      const connectors = await publicRelay.connectors();
      if (!current()) return;
      setRelayConnectors(connectors);
      setRelayConnectorsError(undefined);
    } catch (error) {
      if (!current()) return;
      setRelayConnectors([]);
      setRelayConnectorsError(error instanceof Error ? error.message : String(error));
    }
  }, []);

  const expireBrowserRelayPairing = useCallback(() => {
    relayPairingRestartRequested.current = false;
    void relayPairingSession.cancel().catch(() => undefined);
    setRelayBrowserPairing(undefined);
    setRelayPairingError(BROWSER_PAIRING_EXPIRED);
  }, [relayPairingSession]);

  // Never renew an offer automatically. One lifecycle owns polling and expiry
  // and stops both before applying a terminal result or effect cleanup.
  useEffect(() => {
    if (!relayBrowserPairing) return;
    const sessionCurrent = relayPairingSession.checkpoint();
    return startBrowserPairingLifecycle({
      expiresAt: relayBrowserPairing.expiresAt,
      isCurrent: () => relayPanelOpen.current && sessionCurrent() && !relayPairingStartInFlight.current,
      poll: () => publicRelay.browserPairing!.poll(),
      onPaired: () => {
        setRelayBrowserPairing(undefined);
        setNotice("Remote access is connected.");
        void refreshRelayConnectors();
      },
      onExpired: expireBrowserRelayPairing,
      onFailure: (error) => {
        setRelayBrowserPairing(undefined);
        const message = browserPairingPollFailureMessage(error);
        setRelayPairingError(message);
        actionFailed(new Error(message));
      }
    });
  }, [actionFailed, expireBrowserRelayPairing, refreshRelayConnectors, relayBrowserPairing, relayPairingSession]);

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
    if (!relayConnectorMounted.current || relayConnectorMutation.current) return;
    const token = {};
    relayConnectorMutation.current = token;
    relayConnectorRead.current += 1;
    const epoch = relayConnectorMountEpoch.current;
    const current = () => relayConnectorMounted.current && epoch === relayConnectorMountEpoch.current && relayConnectorMutation.current === token;
    try {
      relayPairingRestartRequested.current = false;
      await relayPairingSession.cancel();
      if (!current()) return;
      const status = await unpairRelay();
      if (!current()) return;
      setRelayBrowserPairing(undefined);
      setRelayPairingError(undefined);
      setRelayConnectors([]);
      setNotice(status?.detail ?? "Remote access was disconnected and unpaired.");
    } catch (error) {
      if (current()) actionFailed(error);
    } finally {
      if (relayConnectorMutation.current === token) {
        relayConnectorMutation.current = undefined;
        relayConnectorRead.current += 1;
        if (relayConnectorMounted.current) void refreshRelayConnectors();
      }
    }
  };

  const openRemoteAccountDeletionPage = async () => {
    try {
      await externalLinks.open(relayProfile.accountDeletionInformationUrl);
    } catch {
      setNotice("Could not open the account deletion page. Try again when a browser is available.");
    }
  };

  const retryRelayWithdrawal = async () => {
    if (relayWithdrawalPending || !publicRelay.retryWithdrawal) return;
    setRelayWithdrawalPending(true);
    try {
      const status = await publicRelay.retryWithdrawal();
      setNotice(status.pendingRevocation ? status.detail ?? "Cloud withdrawal is still pending. Try again when online." : "Cloud authorization withdrawal completed.");
    } catch (error) { actionFailed(error); }
    finally { setRelayWithdrawalPending(false); }
  };

  const unpairRelayAccount = async () => {
    if (!await appPrompts.confirm("Unpair this Quad Cortex from Remote MCP Cloud Relay? Linked AI apps will no longer be able to reach it.")) return;
    await unpairPublicRelay();
  };

  const unlinkRelayConnector = async (grantId: string) => {
    const connector = relayConnectors.find((row) => row.grantId === grantId);
    if (!connector || !publicRelay.unlinkConnector || relayUnlinkingGrantId || !relayConnectorMounted.current || relayConnectorMutation.current) return;
    const token = { grantId };
    relayConnectorMutation.current = token;
    relayConnectorRead.current += 1;
    const epoch = relayConnectorMountEpoch.current;
    const current = () => relayConnectorMounted.current && epoch === relayConnectorMountEpoch.current && relayConnectorMutation.current === token;
    setRelayUnlinkingGrantId(grantId);
    try {
      if (!await appPrompts.confirm(`Unlink ${connector.name}? Its Remote MCP authorization will be revoked immediately.`) || !current()) return;
      const connectors = await publicRelay.unlinkConnector(grantId);
      if (!current()) return;
      setRelayConnectors(connectors);
      setRelayConnectorsError(undefined);
      setNotice(`${connector.name} unlinked from Remote MCP.`);
    } catch (error) {
      if (!current()) return;
      actionFailed(error);
      setRelayConnectorsError(error instanceof Error ? error.message : String(error));
    } finally {
      if (relayConnectorMutation.current === token) {
        relayConnectorMutation.current = undefined;
        relayConnectorRead.current += 1;
        if (relayConnectorMounted.current) {
          setRelayUnlinkingGrantId(undefined);
          if (epoch !== relayConnectorMountEpoch.current) void refreshRelayConnectors();
        }
      }
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

  /** Paired status is idle. First-time REMOTE open may start one offer;
   * later pairing/refresh actions must be explicit. Closing withdraws it. */
  const setRemotePanelOpen = useCallback((open: boolean) => {
    if (relayPanelOpen.current === open) return;
    relayPanelOpen.current = open;
    if (open) {
      void refreshRelayConnectors();
      if (relayStatus?.paired === false && relayPairingStartInFlight.current) relayPairingRestartRequested.current = true;
      beginBrowserPairingOnPanelOpen(relayStatus?.paired, !!relayBrowserPairing, relayPairingStarting, () => void beginBrowserRelayPairing());
      return;
    }
    relayPairingRestartRequested.current = false;
    void relayPairingSession.cancel().catch(() => undefined);
    setRelayBrowserPairing(undefined);
  }, [beginBrowserRelayPairing, refreshRelayConnectors, relayBrowserPairing, relayPairingSession, relayPairingStarting, relayStatus?.paired]);

  useEffect(() => () => {
    relayPanelOpen.current = false;
    relayPairingRestartRequested.current = false;
    void relayPairingSession.cancel().catch(() => undefined);
  }, [relayPairingSession]);

  const chooseScene = performanceWorkflow.selectScene;
  const toggleSelectedBypass = useCallback(() => {
    const block = snapshot.blocks.find((candidate) => candidate.id === selectedBlockId);
    if (block) void performanceWorkflow.toggleBlockBypass(block);
  }, [performanceWorkflow, selectedBlockId, snapshot.blocks]);
  const pressFootswitch = performanceWorkflow.pressFootswitch;
  const deviceViews = useQcDeviceViews({ ...deviceWorkflowSession,
    destination: screenDestination, snapshot, gateway: activeGateway, notice: setNotice, fail: actionFailed,
    session: { demo: demoSession, hardwareReady: deviceReady, sessionReady },
    workflows: { performance: performanceWorkflow, tuner: tunerWorkflow, looper: looperWorkflow, ioEq: ioEqWorkflow, preset: presetWorkflow, deviceSettings: deviceSettingsWorkflow }
  });
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
    rotation: { editor: adjustEditorParameter, tempo: adjustTempo, masterVolume: adjustMasterVolume },
    demo: demoSession,
    notice: setNotice
  });

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (appPromptActive) return;
      if (event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      if (appMenuOpen || sceneWorkflow.command || sceneWorkflow.labelColorOpen || deviceSettingsWorkflow.open || presetDirectoryOpen || addDevicePickerOpen || tempoWorkflow.open) return;
      // The Grid destinations that fill the screen: their Grid and its
      // device shortcuts are not on screen. Gig View, Tuner and Looper keep
      // their single-key footswitch shortcuts.
      if (deviceScreenView === "modes" || deviceScreenView === "midi-out" || deviceScreenView === "cpu-monitor" || presetWorkflow.saveProps.open) return;
      // Escape closes the block editor even when focus is on one of its
      // controls. Nested menus/dialogs get first refusal through their own
      // key handler (defaultPrevented) or their accessible scope.
      if (shouldCloseBlockEditorOnEscape(event, Boolean(blockDetails))) {
        event.preventDefault();
        parameterWorkflow.cancel();
        void gridWorkflow.closeOnDevice();
        return;
      }
      const textEntry = target?.matches("input, textarea, [contenteditable=true]");
      if (textEntry) return;
      if (target?.matches("button, select")) return;
      if (blockDetails) {
        if (footswitchAssignmentPending && /^[1-8]$/.test(event.key)) {
          event.preventDefault();
          const index = Number(event.key) - 1;
          const assigned = snapshot.blocks.find((block) => block.id === selectedBlockId)?.footswitch;
          setFootswitchAssignmentPending(false);
          void applyFootswitchAssignment(assigned === index ? null : index);
          return;
        }
        return;
      }
      if (/^[1-8]$/.test(event.key)) event.ctrlKey ? void chooseScene(Number(event.key) - 1) : void pressFootswitch(Number(event.key) - 1);
      if (event.key === "[") void navigateBank(-1);
      if (event.key === "]") void navigateBank(1);
      if (event.key.toLowerCase() === "t") event.shiftKey ? void deviceViews.open("tuner") : tapTempo();
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
  }, [appPromptActive, addDevicePickerOpen, appMenuOpen, applyFootswitchAssignment, blockDetails, chooseScene, deviceScreenView, deviceSettingsWorkflow, footswitchAssignmentPending, moveBlockSelection, navigateBank, openBlockEditor, parameterWorkflow.cancel, gridWorkflow.closeOnDevice, presetDirectoryOpen, presetWorkflow.saveProps.open, pressFootswitch, removeSelectedBlock, sceneWorkflow, selectedBlockId, deviceViews.open, snapshot.blocks, tapTempo, tempoWorkflow, toggleSelectedBypass]);

  /**
   * Leave the demo device, if it is active, for the real one. Only the route
   * changes: the demo store stays on disk exactly as last saved, and the next
   * "Work with a demo device" reopens it from there.
   */
  const suspendDemoDevice = (): boolean => {
    if (demoDeviceSession.source !== "virtual") return false;
    demoDeviceSession.useDevice();
    demoCanYieldToDevice.current = false;
    // Undo entries replay against the transport that recorded them; none may
    // cross from the demo device to the hardware or back.
    clearUndo();
    presetWorkflow.resetCache();
    setSelectedBlockId("");
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-suspended", result: "info", detail: "The demo device was set aside; its presets stay saved on this computer." }]);
    return true;
  };

  // A provider exists only while Demo is explicitly selected. Invalidate its
  // dispatcher immediately and await native retirement before physical work.
  const withdrawDemoProvider = async (clearSelection = false) => {
    const cleanup = demoBrokerCleanup.current;
    const heldSelection = explicitDemoHeld.current || Boolean(cleanup) || Boolean(demoBrokerTransport.current);
    demoBrokerCleanup.current = undefined;
    demoBrokerTransport.current = undefined;
    demoBrokerReady.current = false;
    const closing = cleanup?.();
    const retirement = Promise.all([demoBrokerRetirement.current, closing]).then(() => undefined);
    // Report this attempt's failure, but do not poison every future selection.
    demoBrokerRetirement.current = retirement.catch(() => undefined);
    await retirement;
    if (clearSelection && heldSelection && window.__TAURI_INTERNALS__) {
      demoSelectionObservationEpoch.current += 1;
      demoSelectionChanges.current += 1;
      try { await callTauri("clear_demo_selection"); }
      finally { demoSelectionChanges.current -= 1; demoSelectionObservationEpoch.current += 1; }
    }
  };
  const showDisconnected = (reason: string, keepExplicitDemo = false, providerAlreadyRetiring = false) => {
    if (!keepExplicitDemo) demoDeviceSession.useDevice();
    if (!keepExplicitDemo && !providerAlreadyRetiring) void withdrawDemoProvider().catch(error => setConnectionEvents(events => [...events, {
      at: new Date().toISOString(), event: "demo-provider-withdraw-failed", result: "failure", detail: String(error)
    }]));
    nativeStateAvailable.current = false;
    setDeviceHeldInBackground(false);
    stopSyncProgressAnimation();
    showSyncProgress(null);
    resetCommands();
    clearUndo();
    presetWorkflow.resetCache();
    setSelectedBlockId("");
    setDeviceScreenView(null);
    screenDestination.setPopup(null);
    setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
    const disconnected = qcDisconnectedConnection(reason);
    connectionRef.current = disconnected;
    setConnection(disconnected);
    setNotice(disconnected.detail);
  };
  showDisconnectedRef.current = showDisconnected;

  const observeDemoBrokerEpoch = async (instance: string | null | undefined, observedEpoch = demoSelectionObservationEpoch.current) => {
    if (observedEpoch !== demoSelectionObservationEpoch.current || demoSelectionChanges.current !== 0
        || observeDemoSelectionHold({ held: explicitDemoHeld.current, instance: demoBrokerInstance.current }, { instance }).kind !== "restart") return false;
    // Compare a positive observation to the last registration ACK, never to
    // an Unknown/absent poll. End the old attempt before retiring its listener,
    // so delayed registration/status callbacks cannot reselect its Demo.
    connectionAttempts.invalidate();
    explicitDemoHeld.current = false;
    demoBrokerInstance.current = undefined;
    userDisconnected.current = null;
    awaitingDetection.current = false;
    waitingConnect.current = null;
    reconnectAfterCurrentAttempt.current = false;
    demoCanYieldToDevice.current = false;
    demoSelectionObservationEpoch.current += 1;
    const selectionEpoch = demoSelectionObservationEpoch.current;
    const detail = "The broker restarted; Demo ended; the unit is available to your AI client.";
    setDemoRestartNotice(detail);
    showDisconnected(detail, false, true);
    // Retire only the old provider. Never clear or take over the new broker's
    // selection, start a broker, or open Demo as a result of this observation.
    await withdrawDemoProvider();
    setConnectionEvents(events => [...events, { at: new Date().toISOString(), event: "demo-ended-broker-restart", result: "warning", detail }]);
    return { selectionEpoch };
  };

  const observeHeldDemoSelection = (held: boolean | undefined, instance: string | null | undefined, observedEpoch = demoSelectionObservationEpoch.current) => {
    // Missing/failed observation is Unknown; even authoritative false does
    // not undo a local user's hold. Only an acknowledged explicit choice can.
    if (observedEpoch !== demoSelectionObservationEpoch.current || demoSelectionChanges.current !== 0) return;
    const observation = observeDemoSelectionHold({ held: explicitDemoHeld.current, instance: demoBrokerInstance.current }, { held, instance });
    if (observation.kind !== "adopt") return;
    explicitDemoHeld.current = true;
    demoBrokerInstance.current = observation.instance;
    if (demoDeviceSession.source !== "virtual") {
      showDisconnectedRef.current("Demo was selected by an editor. Select Demo to recover it or explicitly choose the Quad Cortex.");
    }
  };

  // `userAction` is set only for the user's own Connect, USB, device or menu click:
  // it supersedes a recorded Quit first. Mount and device detection leave it unset
  // and keep respecting the Quit.
  const connect = async (mode: "reconnect" | "reset" = "reconnect", physicalId?: string, userAction = false) => {
    if (!userAction && explicitDemoHeld.current) {
      showDisconnected("Demo remains selected. Choose Demo to recover it or explicitly select the Quad Cortex.");
      return;
    }
    if (physicalId && (commandPending || demoPending || awaitingDetection.current || connectionRef.current.phase === "syncing")) return;
    if (userAction) {
      userDisconnected.current = null;
      followState.current = qcFollowAfterUserConnect(followState.current);
    }
    const attempt = connectionAttempts.begin();
    // Tried from a working demo session: the demo's preset must not sit on
    // screen as if it were still active while the hardware is being reached,
    // so the Grid shows a neutral placeholder until one of the two replaces it.
    const fromDemo = suspendDemoDevice();
    if (!fromDemo) demoDeviceSession.useDevice();
    if (fromDemo) setSnapshot(CONNECTING_PLACEHOLDER_SNAPSHOT);
    // A full handshake here always ends with source "device" one way or the
    // other (ready or disconnected); either way a device
    // held in the background is no longer a distinct state from this attempt on.
    setDeviceHeldInBackground(false);
    resetCommands();
    setConnectionPending(false);
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
    waitingConnect.current = { attempt: ++connectSerial.current, fromDemo, current: attempt.current };
    const discovering = { phase: "discovering" as const, detail: "Looking for a Quad Cortex…", demo: true, source: "device" as const };
    connectionRef.current = discovering;
    setConnection(discovering);
    setNotice(discovering.detail);
    try {
      await withdrawDemoProvider(userAction);
      if (!attempt.current()) return;
      if (userAction) explicitDemoHeld.current = false;
      if (userAction) await brokerDevice.acknowledgeUserConnect();
      if (!attempt.current()) return;
      if (physicalId && physicalId !== selectedPhysicalRef.current) {
        const previousId = selectedPhysicalRef.current;
        selectedPhysicalRef.current = physicalId;
        setSelectedPhysicalId(physicalId);
        clearUndo();
        presetWorkflow.resetCache();
        setSnapshot(CONNECTING_PLACEHOLDER_SNAPSHOT);
        try { await physicalDevices.select(physicalId); }
        catch (error) {
          if (!attempt.current()) return;
          selectedPhysicalRef.current = previousId;
          setSelectedPhysicalId(previousId);
          throw error;
        }
        if (!attempt.current()) return;
      }
      reconnectsOutstanding.current += 1;
      let next: Awaited<ReturnType<typeof tauriTransport.reconnect>>;
      try { next = mode === "reset" ? await tauriTransport.resetSession() : await tauriTransport.reconnect(); }
      finally { reconnectsOutstanding.current -= 1; }
      if (!attempt.current()) return;
      awaitingDetection.current = false;
      if (next.phase === "ready") {
        reconnectAfterCurrentAttempt.current = false;
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
        const verifiedRuntime = await waitForSynchronizedQc(
          () => tauriTransport.runtimeStatus(),
          attempt.current
        );
        if (!attempt.current()) return;
        if (!verifiedRuntime) throw new Error("The Quad Cortex connection attempt was superseded before synchronization");
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
        if (next.phase === "disconnected" || next.phase === "searching") {
          showDisconnected(next.detail);
        } else {
          // Preserve real fault and in-progress phases. Neither may restore
          // Demo, and a failed device must not display the old Demo preset.
          if (next.phase === "needs-attention" || next.phase === "degraded") setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
          connectionRef.current = next;
          setConnection(next);
          setNotice(next.detail);
        }
      }
    } catch (error) {
      if (!attempt.current()) return;
      awaitingDetection.current = false;
      const detail = error instanceof Error ? error.message : String(error);
      stopSyncProgressAnimation();
      showSyncProgress(null);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "connection-failed", result: "failure", detail }]);
      showDisconnected(detail);
    }
  };

  const disconnectDevice = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("host");
    if (!operationLease) { return; }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if (commandPending) {
        setNotice("Wait for the current device command to finish before disconnecting.");
        return;
      }
      userDisconnected.current = qcLatchUserDisconnect(brokerTruth.current);
      // Nothing stays held behind a demo once the user disconnects.
      setDeviceHeldInBackground(false);
      demoDeviceSession.useDevice();
      const attempt = connectionAttempts.begin();
      resetCommands();
      awaitingDetection.current = false;
      stopSyncProgressAnimation();
      showSyncProgress(null);
      const disconnecting = { phase: "disconnected" as const, detail: "Closing the Quad Cortex session…", demo: true, source: "device" as const };
      // Revoke Ready before closing USB. This also prevents the liveness monitor
      // from misclassifying an intentional close as an unexpected device loss.
      connectionRef.current = disconnecting;
      setConnection(disconnecting);
      setNotice(disconnecting.detail);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnect-started", result: "pending", detail: "Closing the live device and gateway session." }]);
      try {
        await withdrawDemoProvider(true);
        if (!attempt.current() || !operationCurrent()) return;
        explicitDemoHeld.current = false;
        const next = await brokerDevice.disconnect();
        if (!attempt.current() || !operationCurrent()) return;
        nativeStateSequence.current = 0;
        nativeStateStream.current = undefined;
        gridRepair.current = createGridRepairCoordinator();
        nativeStateAvailable.current = false;
        liveSyncFailures.current = 0;
        resetCommands();
        // A session the broker left open for another client never closed
        // (qcDisconnectOutcome): the latch stays held so no follow bounces
        // the window through a reconnect, and Live is restored below without
        // one, the way resumeDeviceSession resumes a background session.
        const outcome = qcDisconnectOutcome(userDisconnected.current, next);
        userDisconnected.current = outcome.latch;
        connectionRef.current = outcome.connection;
        setConnection(outcome.connection);
        setNotice(outcome.notice);
        if (outcome.closed) {
          showDisconnected(outcome.notice);
          setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnected", result: "success", detail: outcome.notice }]);
          return;
        }
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnect-left-open", result: "warning", detail: outcome.notice }]);
        try {
          const verifiedRuntime = await tauriTransport.runtimeStatus();
          if (!attempt.current() || !operationCurrent()) return;
          if (!runtimeHasSynchronizedQc(verifiedRuntime)) {
            throw new Error(verifiedRuntime.usbDiagnostics?.detail || "The Quad Cortex preset is not synchronized");
          }
          const current = await tauriTransport.currentSnapshot();
          if (!attempt.current() || !operationCurrent()) return;
          setRuntime(verifiedRuntime);
          clearUndo();
          presetWorkflow.resetCache();
          setSnapshot(current);
          setSelectedBlockId("");
          const ready = { phase: "ready" as const, detail: outcome.notice, demo: false, source: "device" as const, lastSync: new Date().toISOString() };
          // The session never closed, so there is no Disconnect left to hold.
          userDisconnected.current = null;
          connectionRef.current = ready;
          setConnection(ready);
          setNotice(outcome.notice);
          setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "device-kept-live", result: "warning", detail: `${outcome.notice}. The Quad Cortex stays live in this window; no reconnect was needed.` }]);
        } catch (error) {
          if (!attempt.current() || !operationCurrent()) return;
          // Not restored: the latch stays held and the reason stays on screen.
          const detail = error instanceof Error ? error.message : String(error);
          setNotice(outcome.notice);
          setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "device-kept-live-failed", result: "warning", detail: `${outcome.notice}. Live could not be restored without a reconnect: ${detail}` }]);
        }
      } catch (error) {
        if (!attempt.current() || !operationCurrent()) return;
        const detail = error instanceof Error ? error.message : String(error);
        const failed = { phase: "needs-attention" as const, detail, demo: true, source: "device" as const };
        connectionRef.current = failed;
        setConnection(failed);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "disconnect-failed", result: "failure", detail }]);
        setNotice(detail);
      }
    } finally { operationLease.release(); }
  };

  // The macOS status item stays available while the main window is hidden.
  // Route its actions through the same user workflows as the in-window
  // controls so selection, Quit acknowledgement and connection reporting stay
  // consistent.
  macMenuConnectRef.current = () => { void connect("reconnect", undefined, true); };
  macMenuDisconnectRef.current = () => { void disconnectDevice(); };
  useEffect(() => {
    let disposed = false;
    const unlisten: Array<() => void> = [];
    void Promise.all([
      listenTauri<void>("macos-menu-connect", () => macMenuConnectRef.current?.()),
      listenTauri<void>("macos-menu-disconnect", () => macMenuDisconnectRef.current?.())
    ]).then((stops) => {
      if (disposed) {
        stops.forEach((stop) => stop());
        return;
      }
      unlisten.push(...stops);
      if (typeof navigator !== "undefined" && /Mac/.test(navigator.platform)) {
        void macosMenuEventsReady().catch((error: unknown) => {
          console.error("QC Remote could not activate macOS menu actions:", error);
        });
      }
    }).catch((error: unknown) => {
      console.error("QC Remote could not register macOS menu actions:", error);
    });
    return () => {
      disposed = true;
      for (const stop of unlisten) stop();
    };
  }, []);

  /**
   * Open the demo device against its last saved state and route every
   * command to it.
   *
   * Only an explicit Demo selection opens this provider. A person can step
   * away from an already-
   * connected, ready Quad Cortex without disconnecting it by selecting Demo -
   * see `keepDeviceInBackground`
   * below, and resumeDeviceSession for the way back that does not repeat the
   * handshake.
   */
  const startDemoDevice = async () => {
    // A connect() attempt still awaiting its answer owns the next step
    // itself - awaitingDetection.current stays true for the whole discovering
    // /handshaking/syncing window, not just its first phase - exactly like
    // commandPending/demoPending. A ready device does not any more (see the
    // doc comment above), so it is deliberately not part of this guard. Note
    // this must read awaitingDetection rather than a stale render's phase.
    if (commandPending || demoPending || awaitingDetection.current) return;
    const previousDemoHold = { held: explicitDemoHeld.current, instance: demoBrokerInstance.current };
    let registrationStarted = false;
    explicitDemoHeld.current = true;
    demoBrokerInstance.current = undefined; // A new explicit selection binds only its own ACK.
    demoSelectionObservationEpoch.current += 1;
    reconnectAfterCurrentAttempt.current = false;
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
    const closeDeviceSession = qcShouldDisconnectForDemo(connectionRef.current, deviceReady, false);
    setDemoPending(true);
    stopSyncProgressAnimation();
    showSyncProgress(null);
    resetCommands();
    setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-starting", result: "pending", detail: "Opening the demo device saved on this computer." }]);
    try {
      if (closeDeviceSession) await tauriTransport.disconnect().catch(() => undefined);
      if (!attempt.current()) return;
      const opened = await demoDeviceSession.start(virtualDeviceStorage, attempt.current);
      if (!opened || !attempt.current()) return;
      const { transport: demo, connection: state, snapshot: current } = opened;
      let published = false;
      // Every explicit selection renews registration and binds its own ACK.
      // The saved store may be reused, but an earlier attempt's callbacks and
      // epoch must not own this new user selection.
      {
        await withdrawDemoProvider();
        if (!attempt.current()) return;
        registrationStarted = true;
        const cleanupDemoBroker = await exposeDesktopDemo(demo, (ready, message, brokerInstanceId) => {
          if (!attempt.current()) return;
          if (ready && brokerInstanceId && brokerInstanceId !== demoBrokerInstance.current) {
            demoBrokerInstance.current = brokerInstanceId;
            demoSelectionObservationEpoch.current += 1; // Discard inventory started before this ACK.
          }
          demoBrokerReady.current = ready;
          if (ready && published && attempt.current() && explicitDemoHeld.current && demoDeviceSession.virtual === demo) {
            void demo.currentSnapshot().then(refreshed => {
              if (!attempt.current() || !demoBrokerReady.current || !explicitDemoHeld.current || demoDeviceSession.virtual !== demo) return;
              connectionRef.current = state;
              setConnection(state);
              setSnapshot(refreshed);
              setNotice("Demo recovered in this window and MCP. Nothing is sent to the Quad Cortex.");
            }).catch(() => undefined);
          }
          if (!ready && message && appOperationMounted.current) {
            setConnectionEvents(events => [...events, { at: new Date().toISOString(), event: "demo-mcp-unavailable", result: "info", detail: `Demo access is unavailable: ${message}` }]);
            if (published && demoDeviceSession.source === "virtual" && demoDeviceSession.virtual === demo) {
              // The provider really ended: do not show a usable Demo without
              // MCP, but never turn that failure into an implicit QC choice.
              userDisconnected.current = qcLatchUserDisconnect(brokerTruth.current);
              showDisconnected(message, true);
            }
          }
        }, refreshed => {
          if (appOperationMounted.current && demoDeviceSession.source === "virtual" && demoDeviceSession.virtual === demo) {
            setSnapshot(refreshed);
          }
        }, cleanup => {
          if (attempt.current()) demoBrokerCleanup.current = cleanup;
          else void cleanup().catch(() => undefined);
        });
        if (!attempt.current()) { await cleanupDemoBroker?.(); return; }
        if (window.__TAURI_INTERNALS__ && !demoBrokerReady.current) {
          await cleanupDemoBroker?.();
          throw new Error("The Demo provider was lost before its selection completed");
        }
        demoBrokerCleanup.current = cleanupDemoBroker;
        demoBrokerTransport.current = demo;
      }
      clearUndo();
      presetWorkflow.resetCache();
      nativeStateAvailable.current = false;
      // A hardware mirror cannot render the Demo gateway, which has no
      // framebuffer. Retire physical navigation before publishing Demo data.
      setDeviceScreenView(null);
      screenDestination.setPopup(null);
      // The demo state first, then its content: the demo's preset is never on
      // screen under a device-sourced state, even between two updates.
      connectionRef.current = state;
      published = true;
      setDemoRestartNotice(null);
      demoCanYieldToDevice.current = false;
      setConnection(state);
      setSnapshot(current);
      setSelectedBlockId("");
      setDeviceHeldInBackground(keepDeviceInBackground);
      setNotice(keepDeviceInBackground
          ? `${state.detail} ${current.presetLocation} · ${current.presetName} loaded from this computer. The Quad Cortex stays connected in the background; select its USB tab to return to it.`
          : `${state.detail} ${current.presetLocation} · ${current.presetName} loaded from this computer.`);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-started", result: "success", detail: keepDeviceInBackground ? "Demo device active in this window and MCP. The Quad Cortex stays connected in the background; nothing is sent to it while the demo is active." : "Demo device active in this window and MCP. Edits are saved on this computer; nothing is sent to a Quad Cortex." }]);
    } catch (error) {
      if (!attempt.current()) return;
      const detail = error instanceof Error ? error.message : String(error);
      demoDeviceSession.useDevice();
      await withdrawDemoProvider().catch(() => undefined);
      if (!attempt.current()) return;
      // A worker records its attempted registration before sending it. Only
      // its explicit no-attempt proof (or a failure before exposing it) can
      // undo this new hold. Preserve an older selection and all lost ACKs.
      if (!registrationStarted || demoRegistrationNeverAttempted(error)) {
        explicitDemoHeld.current = previousDemoHold.held;
        demoBrokerInstance.current = previousDemoHold.instance;
      }
      const failed = { phase: "needs-attention" as const, detail: `The demo device could not be opened: ${detail}`, demo: true, source: "device" as const };
      setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
      connectionRef.current = failed;
      setConnection(failed);
      setNotice(failed.detail);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "demo-device-failed", result: "failure", detail }]);
    } finally {
      if (demoOpenToken.current === demoOpen) setDemoPending(false);
      if (attempt.current() && reconnectAfterCurrentAttempt.current
          && demoDeviceSession.source === "virtual"
          && demoCanYieldToDevice.current) {
        reconnectAfterCurrentAttempt.current = false;
        // A device seen during the failed attempt: the next inventory poll
        // that reads it ready decides (qcFollowBroker, under its backoff and
        // the user's Disconnect latch); a poll refused after a tray release or
        // Quit decides nothing. Only the live measurement restarts here.
        recordLiveSessionLoss();
      }
    }
  };

  /** "Stop the demo device": go fully idle - no demo, no hardware - keeping the demo's saved state. */
  const stopDemoDevice = () => {
    if (commandPending) {
      setNotice("Wait for the current command to finish before stopping the demo device.");
      return;
    }
    // A QC may have become ready after Demo was opened. Withdrawing Demo
    // selects that broker target, so the window must follow it too, rather
    // than latching an idle view while MCP serves the newly ready hardware.
    const stopOutcome = qcStopDemoOutcome(brokerTruth.current.deviceReady);
    if (stopOutcome.resume) {
      // A Quad Cortex stayed connected behind the demo (see startDemoDevice's
      // keepDeviceInBackground): "stop" has nothing truthful to go idle to,
      // since hardware is still attached and ready, so it resumes that device
      // instead of claiming to disconnect something that is still there.
      void resumeDeviceSession();
      return;
    }
    // A flag left from a device that is no longer live behind the demo.
    if (stopOutcome.clearHeld) setDeviceHeldInBackground(false);
    const stopAttempt = connectionAttempts.begin();
    // Its "disconnected" is the user's idle choice, which a lifted Disconnect
    // latch must not turn into a follow (qcFollowBroker).
    userDisconnected.current = qcLatchUserDisconnect(brokerTruth.current);
    // The demo's preset leaves the Grid with the demo itself, before the
    // device-sourced idle state is shown, so it is never paired with that
    // state or with the error of a later Connect that fails.
    if (suspendDemoDevice()) setSnapshot(IDLE_PLACEHOLDER_SNAPSHOT);
    void withdrawDemoProvider(true).then(() => { if (stopAttempt.current()) explicitDemoHeld.current = false; })
      .catch(error => setNotice(`Demo provider could not be withdrawn: ${String(error)}`));
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
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("host");
    if (!operationLease) { return; }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if ((!deviceHeldInBackground && !brokerTruth.current.deviceReady) || commandPending || demoPending || demoDeviceSession.source !== "virtual") return;
      const attempt = connectionAttempts.begin();
      suspendDemoDevice();
      setSnapshot(CONNECTING_PLACEHOLDER_SNAPSHOT);
      const resuming = { phase: "syncing" as const, detail: "Reading the Quad Cortex that stayed connected in the background…", demo: true, source: "device" as const };
      connectionRef.current = resuming;
      setConnection(resuming);
      setNotice("Reading the Quad Cortex that stayed connected in the background…");
      try {
        await withdrawDemoProvider(true);
        if (!attempt.current() || !operationCurrent()) return;
        explicitDemoHeld.current = false;
        const verifiedRuntime = await tauriTransport.runtimeStatus();
        if (!attempt.current() || !operationCurrent()) return;
        if (!runtimeHasSynchronizedQc(verifiedRuntime)) {
          throw new Error(verifiedRuntime.usbDiagnostics?.detail || "The Quad Cortex preset is not synchronized");
        }
        const current = await tauriTransport.currentSnapshot();
        if (!attempt.current() || !operationCurrent()) return;
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
        if (!attempt.current() || !operationCurrent()) return;
        const detail = error instanceof Error ? error.message : String(error);
        // The background session did not survive. Show Disconnected, never
        // silently reopen Demo after the user chose the physical device.
        setDeviceHeldInBackground(false);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "device-resume-failed", result: "failure", detail }]);
        showDisconnected(detail);
      }
    } finally { operationLease.release(); }
  };

  /** The demo device's store, while the demo device is the active transport. */
  const activeDemoStore = () => demoDeviceSession.source === "virtual" ? virtualDeviceStoreOf(demoDeviceSession.virtual) : undefined;
  const readClipboardText = async (current: () => boolean): Promise<string | undefined> => {
    try {
      return await navigator.clipboard.readText();
    } catch (error) {
      if (current()) setNotice(`QC Remote could not read the clipboard: ${error instanceof Error ? error.message : String(error)}`);
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
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    try {
      if (!sessionReady || commandPending) return;
      const operation = captureDeviceOperation();
      const current = snapshotRef.current;
      const store = activeDemoStore();
      if (!operation.current()) return;
      setNotice(`Copying ${current.presetName || "the preset"} as text…`);
      try {
        const copied = await capturePresetText({ gateway: operation.gateway, snapshot: current, source: store ? "virtual" : "device", demoPreset: store?.currentPreset(), onProgress: (message) => { if (operation.current()) setNotice(message); } });
        operation.assertCurrent();
        await navigator.clipboard.writeText(copied.text);
        operation.assertCurrent();
        setNotice(copied.summary);
      } catch (error) {
        if (!operation.current()) return;
        setNotice(`The preset was not copied: ${error instanceof Error ? error.message : String(error)}`);
      }
    } finally { operationLease.release(); }
  };

  /**
   * Edit > Paste Preset from Text. Into the demo device it replaces the active
   * preset directly; onto a Quad Cortex it is replayed as ordinary block and
   * parameter writes, each guarded as usual, and every refused write is
   * reported. Either way the result is unsaved until the preset is saved.
   */
  const pastePresetFromText = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    try {
      if (!sessionReady || commandPending) return;
      const operation = captureDeviceOperation();
      const text = await readClipboardText(operation.current);
      if (text === undefined || !operation.current()) return;
      const parsed = parsePresetText(text);
      if (!parsed.ok) { setNotice(parsed.message); return; }
      const pasted = parsed.document;
      const pastedName = pasted.snapshot.presetName || "the unnamed preset";
      const target = snapshotRef.current;
      const occupied = target.blocks.filter(isGridDevice).length;
      const store = activeDemoStore();
      if (store) {
        if ((occupied || target.dirty) && !await appPrompts.confirm(`Replace “${target.presetName || "the unnamed preset"}” on the demo device with the pasted “${pastedName}”? Its ${occupied} device${occupied === 1 ? "" : "s"}${target.dirty ? " and unsaved changes" : ""} will be replaced. This is temporary until the preset is saved.`)) return;
        if (!operation.current()) return;
        try {
          if (snapshotRef.current.presetIdentity !== target.presetIdentity) return;
          const result = await pastePresetIntoDemo(store, pasted);
          operation.assertCurrent();
          clearUndo();
          const current = await operation.gateway.currentSnapshot();
          operation.assertCurrent();
          reconcileWorkflowSnapshot(current);
          setNotice(result.summary);
        } catch (error) {
        if (!operation.current()) return;
          setNotice(`The preset was not pasted: ${error instanceof Error ? error.message : String(error)}`);
        }
        return;
      }
      if (!deviceReady) return;
      // A paste is a full overwrite once confirmed. A copy from the demo device
      // always asks first, saying that its untouched values are approximations.
      const confirmation = presetReplayConfirmation(pasted, target);
      if (confirmation !== undefined && !await appPrompts.confirm(confirmation)) return;
      if (!operation.current()) return;
      try {
        const report = await replayPresetOnDevice({ gateway: operation.gateway, document: pasted, target, isConnectionLost: isQcConnectionFailure, onProgress: (message) => { if (operation.current()) setNotice(message); } });
        operation.assertCurrent();
        clearUndo();
        try {
          const current = await operation.gateway.currentSnapshot();
          operation.assertCurrent();
          reconcileWorkflowSnapshot(current);
        } catch { operation.assertCurrent(); if (report.snapshot) reconcileWorkflowSnapshot(report.snapshot); }
        setNotice(presetReplaySummary(report, pasted));
      } catch (error) {
        if (!operation.current()) return;
        actionFailed(error);
      }
    } finally { operationLease.release(); }
  };

  /**
   * Edit > Copy Setlist records the authoritative source. A physical source is
   * also captured preset-by-preset so it can be pasted into the Demo device;
   * the firmware's atomic duplicate remains the fast path on the same QC.
   */
  const copySetlist = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    try {
      if (!sessionReady || commandPending) return;
      const operation = captureDeviceOperation();
      try {
        const identity = await operation.gateway.identity();
        operation.assertCurrent();
        const source = connection.source === "virtual" ? "virtual" : "device";
        let captured: CapturedSetlist | undefined;
        if (source === "device") {
          if (!await appPrompts.confirm(`Copy all occupied presets in setlist “${snapshot.setlistName}”? QC Remote will briefly recall each preset and each of its eight scenes, read it, then restore ${snapshot.presetLocation} · ${snapshot.presetName}. Nothing on the Quad Cortex will be saved or overwritten.`)) return;
          if (!operation.current()) return;
          captured = await captureSetlistForDemo({
            gateway: operation.gateway,
            setlistKey: snapshot.setlistKey,
            setlistName: snapshot.setlistName,
            onProgress: (message) => { if (operation.current()) setNotice(message); }
          });
          operation.assertCurrent();
          const current = await operation.gateway.currentSnapshot();
          operation.assertCurrent();
          reconcileWorkflowSnapshot(current);
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
        if (!operation.current()) return;
        actionFailed(error);
      }
    } finally { operationLease.release(); }
  };

  /** Edit > Paste Setlist uses native duplication on one device and reconstruction across device boundaries. */
  const pasteSetlist = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    try {
      if (!sessionReady || commandPending || !setlistClipboard) return;
      const operation = captureDeviceOperation();
      try {
        const identity = await operation.gateway.identity();
        operation.assertCurrent();
        const source = connection.source === "virtual" ? "virtual" : "device";
        if (source === "virtual" && setlistClipboard.source === "device" && setlistClipboard.captured) {
          const store = activeDemoStore();
          if (!store) throw new Error("The Demo device store is not available.");
          if (!await appPrompts.confirm(`Import ${setlistClipboard.captured.presets.length} occupied presets from “${setlistClipboard.setlistName}” into the Demo device? They keep their original slot positions. Unsupported Capture or IR content will be reported and left out.`)) return;
          if (!operation.current()) return;
          const imported = await importCapturedSetlistIntoDemo(store, setlistClipboard.captured);
          operation.assertCurrent();
          presetWorkflow.resetCache();
          setNotice(`Imported “${imported.name}” into the Demo device with ${imported.presets} presets.${imported.skipped.length ? ` ${imported.skipped.length} unsupported block${imported.skipped.length === 1 ? " was" : "s were"} left out.` : " Every captured block was supported."}`);
          return;
        }
        if (!operation.current()) return;
        if (identity.serial !== setlistClipboard.serial || source !== setlistClipboard.source) {
          setNotice("That setlist came from another target. Physical-to-Demo transfer is supported; writing a reconstructed setlist onto a different physical Quad Cortex is not enabled yet.");
          return;
        }
        if (!await appPrompts.confirm(`Duplicate setlist “${setlistClipboard.setlistName}” on this ${source === "virtual" ? "demo device" : "Quad Cortex"}? This creates a persistent copy.`)) return;
        if (!operation.current()) return;
        const result = await operation.gateway.duplicateSetlist(setlistClipboard.setlistKey, setlistClipboard.setlistName);
        operation.assertCurrent();
        presetWorkflow.resetCache();
        setNotice(result.detail);
      } catch (error) {
        if (!operation.current()) return;
        actionFailed(error);
      }
    } finally { operationLease.release(); }
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
    // Details are for the on-screen log only: they can contain preset names,
    // filesystem paths and native error text. Copy/issue must be as private as export.
    events: connectionEvents.slice(-100).map(({ at, event }) => ({ at, event }))
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
    if (corpusFixtureEnabled) return;
    autoConnectStarted.current = true;
    // Startup follows real broker state. No simulated provider is registered
    // until the user explicitly selects Demo.
    void connect();
    return () => { autoConnectStarted.current = false; };
  }, []);

  // The broker's own phase is the evidence a device is there. It is pushed,
  // because device.reconnect holds the gateway for the whole handshake and
  // nothing can be asked of it meanwhile. Read through a ref so a detection
  // long after mount still runs the current render's connect.
  const connectOnDetection = useRef(connect);
  connectOnDetection.current = connect;
  useEffect(() => onDevicePhase(({ phase, detail, deviceId }) => {
    // Legacy/shared-pool phase notifications can omit deviceId. They still
    // describe the selected controller; only reject an explicit different id.
    if (!qcShouldFollowDevicePhase({ phase, deviceId }, selectedPhysicalRef.current, qcUserDisconnectHolds(userDisconnected.current))) return;
    if (awaitingDetection.current) {
      reconnectAfterCurrentAttempt.current = true;
      beginSyncProgress();
      const detected = qcConnectionDuringBrokerPhase(phase, detail);
      if (detected && (connectionRef.current.phase !== detected.phase || connectionRef.current.detail !== detected.detail)) {
        connectionRef.current = detected;
        setConnection(detected);
        setNotice(detected.detail);
        setSnapshot(connectingPlaceholderSnapshot(detected.phase === "opening" ? "opening" : detected.phase === "handshaking" ? "handshaking" : "synchronizing"));
      }
      return;
    }
    // Plugged in after a connect had already given up: the broker keeps
    // looking on its own, so follow it rather than waiting for a click. Once
    // an inventory poll has answered, the poll decides from what the broker
    // reports (it follows a ready device within 2 s, and never after a tray
    // release or Quit); before that, for a broker the poll cannot read, this
    // push decides through the same latch-aware decision.
    if (qcPhaseListenerMayFollow(inventoryPollAnswered.current)) followReadyInventory.current({ kind: "answered", inventoryReady: true });
  }), []);
  // The watchdog for a connection nothing will finish: a connect() whose
  // device.reconnect never answers holds the host's gateway, and with it the
  // inventory poll, for up to the host's five-minute timeout; a "discovering"
  // fallback whose demo open was refused is owned by nothing at all. Neither
  // connects here. Read through a ref, like the follow, so it sees this
  // render's pending state.
  const watchConnectStall = useRef<() => void>(() => undefined);
  watchConnectStall.current = () => {
    const waiting = waitingConnect.current;
    const step = qcWatchConnectStall({
      connection: connectionRef.current,
      connect: awaitingDetection.current && waiting?.current() ? { attempt: waiting.attempt, fromDemo: waiting.fromDemo } : null,
      otherWorkPending: commandPending || demoPending,
      stall: connectStall.current,
      now: performance.now()
    });
    connectStall.current = step.stall;
    if (step.abandon === "disconnected") {
      // Supersede it first, so its late answer is ignored whatever happens
      // next; drop a retry it collected; and record the loss, which the
      // follow (qcFollowBroker) answers only after the abandoned request has settled
      // (reconnectsOutstanding keeps every automatic follow held until then).
      connectionAttempts.begin();
      awaitingDetection.current = false;
      reconnectAfterCurrentAttempt.current = false;
      stopSyncProgressAnimation();
      showSyncProgress(null);
      recordLiveSessionLoss();
      const detail = `the Quad Cortex did not answer within ${QC_CONNECT_STALL_MS / 1000} s`;
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "connection-stalled", result: "failure", detail }]);
      showDisconnected(detail);
    }
  };
  useEffect(() => {
    const timer = window.setInterval(() => watchConnectStall.current(), 2000);
    return () => window.clearInterval(timer);
  }, []);
  // Every settled inventory poll (and, before any poll answered, a detection
  // push) updates broker truth and decides whether the window follows it.
  followReadyInventory.current = (observation) => {
    const truth = qcObserveBroker(brokerTruth.current, observation);
    if (!qcSameBrokerTruth(brokerTruth.current, truth)) setBrokerTruthView(truth);
    brokerTruth.current = truth;
    userDisconnected.current = qcUserDisconnectAfter(userDisconnected.current, truth);
    if (explicitDemoHeld.current) return;
    const step = qcFollowBroker({
      follow: followState.current,
      truth,
      now: performance.now(),
      userDisconnect: userDisconnected.current,
      connectHeld: qcAutomaticConnectHeld({ awaitingDetection: awaitingDetection.current, commandPending, demoPending, reconnectsOutstanding: reconnectsOutstanding.current }),
      connection: connectionRef.current,
      demoCanYield: demoCanYieldToDevice.current
    });
    followState.current = step.follow;
    userDisconnected.current = step.userDisconnect;
    // Never over the user's own Demo choice: there the window was not trying.
    if (step.demoGaveUp && demoCanYieldToDevice.current && !followDemoGaveUpNoticed.current) {
      followDemoGaveUpNoticed.current = true;
      const detail = `The broker reports the Quad Cortex ready, but ${QC_FOLLOW_DEMO_ATTEMPTS} automatic connections did not synchronize it. Staying on the demo device; select its USB tab or Connect to try again.`;
      setNotice(detail);
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "automatic-connect-paused", result: "warning", detail }]);
    } else if (!step.demoGaveUp) followDemoGaveUpNoticed.current = false;
    if (step.connect) void connectOnDetection.current();
  };

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
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if (!deviceReady || commandPending) return;
      setNotice("Refreshing complete device state…");
      setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-started", result: "pending", detail: "Reading the complete active preset state from the device." }]);
      try {
        const current = await qcController.readSnapshot(() => tauriTransport.currentSnapshot());
        if (!operationCurrent()) return;
        setConnection((state) => ({ ...state, lastSync: new Date().toISOString() }));
        setNotice("Live preset state refreshed.");
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-complete", result: "success", detail: `${current.presetLocation} · ${current.presetName} refreshed successfully.` }]);
      } catch (error) {
        if (!operationCurrent() || error instanceof SupersededPresetOperation) return;
        const detail = error instanceof Error ? error.message : String(error);
        setConnectionEvents((events) => [...events, { at: new Date().toISOString(), event: "refresh-failed", result: "failure", detail }]);
        actionFailed(error);
      }
    } finally { operationLease.release(); }
  };

  const saveWorkspace = async (saveAs = false): Promise<{ ok: boolean; message: string }> => {
    if (!appOperationMounted.current || connectionPending) { return { ok: false, message: "Another operation is still running. Try again when it finishes." }; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("host");
    if (!operationLease) { return { ok: false, message: "Another operation is still running. Try again when it finishes." }; }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if (commandPending) return { ok: false, message: "Another operation is still running. Try again when it finishes." };
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
          if (!operationCurrent()) return { ok: true, message: `Workspace saved: ${result.name}` };
          setWorkspacePath(result.path);
          setWorkspaceName(result.name);
          setLoadedWorkspace(document);
          const saved = `Workspace saved: ${result.name}`;
          setNotice(saved);
          return { ok: true, message: saved };
        }
        return { ok: true, message: "Save cancelled. No copy was written." };
      } catch (error) {
        if (operationCurrent()) actionFailed(error);
        return { ok: false, message: error instanceof Error ? error.message : String(error) };
      }
    } finally { operationLease.release(); }
  };

  const createDeviceBackup = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("session");
    if (!operationLease) { return; }
    try {
      if (!deviceReady || commandPending) return;
      const operation = captureDeviceOperation();
      const today = new Date().toISOString().slice(0, 10);
      const name = (await appPrompts.prompt("Backup name", `QC Device Backup ${today}`))?.trim();
      if (!name || !operation.current()) return;
      setNotice("Choose where to save the native backup. The device transfer can take about 40 seconds.");
      try {
        const result = await tauriTransport.createDeviceBackup(name);
        operation.assertCurrent();
        setNotice(formatNativeBackupSaveNotice(result));
      } catch (error) {
        if (operation.current()) actionFailed(error);
      }
    } finally { operationLease.release(); }
  };

  const saveWorkspaceCopy = async (documentId: unknown): Promise<string> => {
    if (typeof documentId !== "string" || !documentId || documentId !== workspaceDocumentIdentity.current?.id || !loadedWorkspace) {
      throw new Error("The workspace displayed by this window is no longer current. Wait for its updated details, then save the copy again.");
    }
    if (!appOperationMounted.current || connectionPending) { throw new Error("Another operation is still running. Try again when it finishes."); }
    const operationLease = qcController.lifetime.mutations.tryAcquire("host");
    if (!operationLease) { throw new Error("Another operation is still running. Try again when it finishes."); }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if (commandPending) throw new Error("Another operation is still running. Try again when it finishes.");
      {
        const result = await saveOpenedWorkspaceCopy(workspaceFiles, loadedWorkspace);
        if (result.cancelled) return "Save cancelled. No copy was written.";
        if (operationCurrent() && result.path) {
          setWorkspacePath(result.path);
          setWorkspaceName(result.name);
        }
        const message = `Workspace copy saved: ${result.name ?? result.path ?? "copy"}`;
        if (operationCurrent()) setNotice(message);
        return message;
      }
    } finally { operationLease.release(); }
  };

  const openWorkspace = async () => {
    if (!appOperationMounted.current || connectionPending) { return; }
    const operationLease = qcController.lifetime.mutations.tryAcquire("host");
    if (!operationLease) { return; }
    const operationEpoch = appOperationEpoch.current;
    const operationCurrent = () => appOperationMounted.current && operationEpoch === appOperationEpoch.current && operationLease.current();
    try {
      if (commandPending) return;
      try {
        const result = await workspaceFiles.open();
        if (operationCurrent() && !result.cancelled && result.document && result.path) {
          setWorkspacePath(result.path);
          setWorkspaceName(result.name);
          setLoadedWorkspace(result.document);
          void appWindows.open("workspace").catch(actionFailed);
          setNotice(`Workspace opened: ${result.name}`);
        }
      } catch (error) {
        if (operationCurrent()) actionFailed(error);
      }
    } finally { operationLease.release(); }
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
    dirty: snapshot.dirty,
    information: deviceViews.deviceInformation
  }), [deviceViews.deviceInformation, snapshot.deviceName, deviceConnectionLabel, snapshot.setlistName, snapshot.presetLocation, snapshot.presetName, snapshot.mode, snapshot.activeScene, snapshot.tempo, snapshot.blocks.length, snapshot.dirty]);
  const workspaceState = useMemo<WorkspaceState>(() => loadedWorkspace ? {
    documentId: workspaceDocumentIdentity.current!.id,
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
  // The runtime status is replaced every poll; keying the summary on its text
  // keeps the Settings window from being re-sent identical state.
  const updaterRelayKey = JSON.stringify(summarizeUpdaterRelay(runtime?.usbDiagnostics?.updaterRelay));
  const updaterRelaySummary = useMemo(() => JSON.parse(updaterRelayKey) as SettingsState["updaterRelay"], [updaterRelayKey]);
  const settingsState = useMemo<SettingsState>(() => ({
    ...brokerTrayStatus,
    updaterRelay: updaterRelaySummary,
    formFactorId,
    formFactors: formFactors.map((item) => ({ id: item.id, displayName: item.displayName })),
    controlAccessMode,
    followQcScreen,
    localAccessCondition: localAccessCondition ? { tone: localAccessCondition.tone, message: localAccessCondition.message, detail: localAccessCondition.detail } : null
  }), [brokerTrayStatus, updaterRelaySummary, formFactorId, controlAccessMode, followQcScreen, localAccessCondition?.tone, localAccessCondition?.message, localAccessCondition?.detail]);
  useEffect(() => publishWindowState(DEVICE_INFO_EVENTS, deviceInfoState), [deviceInfoState]);
  useEffect(() => publishWindowState(WORKSPACE_EVENTS, workspaceState), [workspaceState]);
  useEffect(() => publishWindowState(SETTINGS_EVENTS, settingsState), [settingsState]);

  // Read through a ref, like the diagnostics handlers, so the listeners are
  // attached once and always see the latest render. A Settings change is
  // applied by the same setters the in-app controls call, after checking it
  // names a real device model or access level: an event payload is not typed.
  const appWindowHandlers = useRef({ deviceInfoState, workspaceState, settingsState, changeControlAccessMode, changeFollowQcScreen, saveWorkspaceCopy });
  appWindowHandlers.current = { deviceInfoState, workspaceState, settingsState, changeControlAccessMode, changeFollowQcScreen, saveWorkspaceCopy };
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
      const isMacOS = typeof navigator !== "undefined" && /Mac/.test(navigator.platform);
      if (isMacOS) {
        return status.trayStopping ? "Background broker stopping; it will exit within five seconds."
          : status.trayEnabled ? "Menu bar item and background broker enabled." : "Menu bar item hidden; background broker disabled.";
      }
      return status.trayStopping ? "Broker tray stopping; it will exit within five seconds."
        : status.trayEnabled ? "Tray icon shown." : "Tray icon hidden.";
    },
    setUpdaterRelayEnabled: async (enabled) => {
      if (typeof enabled !== "boolean") throw new Error("Internet for the Quad Cortex must be on or off.");
      const status = await applyBrokerTrayChange(() => brokerTray.setUpdaterRelayEnabled(enabled));
      return status.updaterRelayEnabled
        ? "Internet for the Quad Cortex is on."
        : "Internet for the Quad Cortex is off.";
    },
    setIdleDisconnectMinutes: async (minutes) => {
      const status = await applyBrokerTrayChange(() => brokerTray.setIdleMinutes(minutes));
      return status.idleDisconnectMinutes === null
        ? "Release set to Never. The device stays connected until you choose Disconnect or Quit."
        : status.idleDisconnectMinutes === 0
          ? "Release set to Immediately, about five seconds after the last attached app or AI client disconnects."
          : `Release set to ${status.idleDisconnectMinutes} minutes after the last attached app or AI client disconnects.`;
    },
    saveWorkspaceCopy: async (documentId: unknown) => {
      return appWindowHandlers.current.saveWorkspaceCopy(documentId);
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
    const current = captureLocalStatusRead("claude");
    if (!current()) return { ok: false } as const;
    try {
      const nextStatus = await localClientPackages.claudeDesktopStatus();
      if (!current()) return { ok: false } as const;
      setClaudeExtensionStatus(nextStatus);
      setClaudeStatusRefreshFailed(false);
      if (nextStatus.state === "current" || nextStatus.state === "newer-installed") setClaudeApprovalLapsed(false);
      if (finishPending || nextStatus.state === "current" || nextStatus.state === "newer-installed") {
        setClaudeInstallPending(false);
      }
      return { ok: true, status: nextStatus } as const;
    } catch {
      if (current()) setClaudeStatusRefreshFailed(true);
      return { ok: false } as const;
    }
  }, []);

  const refreshChatgptIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    const current = captureLocalStatusRead("chatgpt");
    if (!current()) return false;
    try {
      const nextStatus = await localClientPackages.chatgptStatus();
      if (!current()) return false;
      setChatgptIntegrationStatus(nextStatus);
      setChatgptStatusRefreshFailed(false);
      setChatgptRestartRequired(nextStatus.restartRequired);
      if (!nextStatus.chatgptRunning) setChatgptRemovalRestartRequired(false);
      return true;
    } catch {
      if (current()) setChatgptStatusRefreshFailed(true);
      return false;
    }
  }, []);

  const refreshAntigravityIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    const current = captureLocalStatusRead("antigravity");
    if (!current()) return false;
    try {
      const nextStatus = await localClientPackages.antigravityStatus();
      if (!current()) return false;
      setAntigravityIntegrationStatus(nextStatus);
      setAntigravityStatusRefreshFailed(false);
      return true;
    } catch {
      if (current()) setAntigravityStatusRefreshFailed(true);
      return false;
    }
  }, []);

  const refreshLmStudioIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    const current = captureLocalStatusRead("lm-studio");
    if (!current()) return false;
    try {
      const nextStatus = await localClientPackages.lmStudioStatus();
      if (!current()) return false;
      setLmStudioIntegrationStatus(nextStatus);
      setLmStudioStatusRefreshFailed(false);
      return true;
    } catch {
      if (current()) setLmStudioStatusRefreshFailed(true);
      return false;
    }
  }, []);

  const refreshJanIntegrationStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    const current = captureLocalStatusRead("jan");
    if (!current()) return false;
    try {
      const nextStatus = await localClientPackages.janStatus();
      if (!current()) return false;
      setJanIntegrationStatus(nextStatus);
      setJanStatusRefreshFailed(false);
      return true;
    } catch {
      if (current()) setJanStatusRefreshFailed(true);
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
    const current = captureLocalStatusRead("access");
    if (!current()) return false;
    try {
      const nextStatus = await localAccess.status();
      if (!current()) return false;
      setLocalAccessStatus(nextStatus);
      return true;
    } catch {
      // An unreadable status is not itself a fault to shout about: the panel
      // simply says nothing rather than inventing a state.
      if (current()) setLocalAccessStatus(undefined);
      return false;
    }
  }, []);

  const refreshOtherMcpStatus = useCallback(async () => {
    if (!window.__TAURI_INTERNALS__) return true;
    const current = captureLocalStatusRead("other");
    if (!current()) return false;
    try {
      const nextStatus = await localClientPackages.otherMcpStdioSetup();
      if (!current()) return false;
      setOtherMcpStatus(nextStatus);
      setOtherMcpStatusRefreshFailed(false);
      return true;
    } catch {
      if (current()) setOtherMcpStatusRefreshFailed(true);
      return false;
    }
  }, []);

  const refreshLocalIntegrations = useCallback(async (finishClaudePending = false) => {
    if (localStatusRefreshInFlight.current) return;
    const current = captureLocalStatusRead("all");
    if (!current()) return;
    localStatusRefreshInFlight.current = true;
    setLocalStatusChecking(true);
    try {
      await Promise.all([
        refreshClaudeExtensionStatus(finishClaudePending),
        refreshChatgptIntegrationStatus(),
        refreshAntigravityIntegrationStatus(),
        refreshLmStudioIntegrationStatus(),
        refreshJanIntegrationStatus(),
        refreshOtherMcpStatus(),
        refreshLocalAccessStatus()
      ]);
    } finally {
      if (current()) {
        setLocalStatusChecking(false);
        localStatusRefreshInFlight.current = false;
      }
    }
  }, [refreshAntigravityIntegrationStatus, refreshChatgptIntegrationStatus, refreshClaudeExtensionStatus, refreshJanIntegrationStatus, refreshLmStudioIntegrationStatus, refreshLocalAccessStatus, refreshOtherMcpStatus]);

  useEffect(() => {
    void refreshLocalIntegrations();
    let current = true;
    let focusTimer: number | undefined;
    const refreshOnFocus = () => {
      if (focusTimer !== undefined) window.clearTimeout(focusTimer);
      focusTimer = window.setTimeout(() => { focusTimer = undefined; if (current) void refreshLocalIntegrations(true); }, 350);
    };
    window.addEventListener("focus", refreshOnFocus);
    return () => {
      current = false;
      window.removeEventListener("focus", refreshOnFocus);
      if (focusTimer !== undefined) window.clearTimeout(focusTimer);
    };
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
      reportLocal({ tone: "action-required", message: "Claude Desktop's download page opened. Install it for this user account, then press Refresh here." });
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
    const hostName = chatgptIntegrationStatus?.clientName ?? "ChatGPT";
    try {
      await localClientPackages.openChatgpt();
      // One sentence in every case. The branch this replaces walked the user
      // through ChatGPT's settings to find a Restart control, which does not
      // exist and which nothing here needs: ChatGPT spawns its MCP servers
      // per chat session, so a new chat is the whole of what makes a
      // rewritten route take effect.
      reportLocal({
        tone: "ready",
        message: hostName === "Codex CLI"
          ? "Codex CLI opened in a terminal. QC Remote is configured as an MCP server for new sessions."
          : "ChatGPT Desktop opened. Start a new chat and type /mcp to confirm QC Remote is connected."
      });
      await refreshChatgptIntegrationStatus();
    } catch (error) {
      failLocal(hostName, error, "Open");
    }
  };

  const openChatgptDesktopDownload = async () => {
    const hostName = chatgptIntegrationStatus?.clientName ?? "ChatGPT Desktop";
    try {
      await localClientPackages.openChatgptDownload();
      reportLocal({ tone: "action-required", message: hostName === "Codex CLI"
        ? "The official Codex CLI documentation opened. Install the CLI, then press Refresh here."
        : "The official ChatGPT Desktop download page opened. Install it for this user account, then press Refresh here." });
    } catch (error) {
      failLocal(hostName, error, `Install ${hostName}`);
    }
  };

  const openChatgptConfiguration = async () => {
    const hostName = chatgptIntegrationStatus?.clientName ?? "ChatGPT";
    try {
      await localClientPackages.openChatgptConfiguration();
      reportLocal({ tone: "action-required", message: hostName === "Codex CLI"
        ? "Codex config.toml opened. Save your changes, then press Refresh here."
        : "The shared ChatGPT and Codex MCP configuration is selected. Fix config.toml, then press Refresh here." });
    } catch (error) {
      failLocal(hostName, error);
    }
  };

  const openAntigravityDownload = async () => {
    try {
      await localClientPackages.openAntigravityDownload();
      reportLocal({ tone: "action-required", message: "The official Google Antigravity download page opened. Install it for this user account, then press Refresh here." });
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
      if (appPromptActive || event.defaultPrevented) return;
      const target = event.target as HTMLElement | null;
      const textEntry = target?.matches("input, textarea, [contenteditable=true]");
      // Modal choices own their keyboard context, even before opening focus
      // has moved on the next animation frame. Never edit the hidden Grid.
      const modalCommandBlocked = sceneWorkflow.command || sceneWorkflow.labelColorOpen
        || target?.closest('[aria-modal="true"]');
      if (modalCommandBlocked && applicationModifier(event) && ["s", "z", "y", "c", "v"].includes(event.key.toLowerCase())) return;
      if (applicationModifier(event) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        if (event.shiftKey) void openDeviceSave();
        else void saveWorkspace();
        return;
      }
      if (!textEntry && applicationModifier(event) && !event.shiftKey && event.key.toLowerCase() === "z") {
        event.preventDefault();
        void undoLastAction();
        return;
      }
      if (!textEntry && applicationModifier(event) && (event.key.toLowerCase() === "y" || (event.shiftKey && event.key.toLowerCase() === "z"))) {
        event.preventDefault();
        void redoLastAction();
        return;
      }
      if (!textEntry && applicationModifier(event) && event.key.toLowerCase() === "c") {
        event.preventDefault();
        void copySelectedBlockSettings();
        return;
      }
      if (!textEntry && applicationModifier(event) && event.key.toLowerCase() === "v") {
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
        if (deviceScreenView) void deviceViews.close();
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
  }, [appPromptActive, blockClipboard, deviceViews.close, commandPending, connection.demo, deviceScreenView, deviceSettingsWorkflow, openDeviceSave, redoEntry, saveWorkspace, sceneWorkflow, selectedBlockId, snapshot, tempoWorkflow, undoEntry]);

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
      case "device-info": void deviceViews.open("info"); break;
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
      case "open-tuner": void deviceViews.toggle("tuner"); break;
      case "open-gig-view": void deviceViews.toggle("gig"); break;
      case "open-io-settings": void deviceViews.toggle("io"); break;
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

  const display = createQcDeviceDisplay({
    captureSessionCurrent: deviceWorkflowSession.captureSessionCurrent,
    screenGestureContextKey: JSON.stringify([connection.source, qcController.lifetime.checkpoint().session, qcController.lifetime.version, snapshot.presetIdentity]),
    snapshot, selectedBlockId, pending: commandPending, destination: screenDestination, views: deviceViews, cpu: cpuTelemetry, undo: deviceUndo,
    workflows: { deviceSettings: deviceSettingsWorkflow, tempo: tempoWorkflow, tuner: tunerWorkflow, looper: looperWorkflow, modes: modesWorkflow, midiOut: midiOutWorkflow, ioEq: ioEqWorkflow, grid: gridWorkflow, scene: sceneWorkflow, preset: presetWorkflow, routing: routingWorkflow },
    mirror: activeGateway, mirrorLifetime: qcController.lifetime,
    selectScene: chooseScene, queueTempo, setParameterMenuInitiallyOpen, notice: setNotice,
    expressionAssignment, laneControlEditor, parameterEditor: parameterEditorBindings,
    openPreset: () => void openPresetBrowser(), refresh: () => void refreshSnapshot(), openRouting: openRoutePicker,
    fixtureScreenView,
    auditContent: productionAuditScreen ? <CorOsProductionScreenAudit view={productionAuditScreen} snapshot={snapshot} /> : undefined
  });

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
      relayPending={relayPending || relayWithdrawalPending}
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
      onRetryRelayWithdrawal={() => void retryRelayWithdrawal()}
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
      deviceHeldInBackground={backgroundDeviceLive}
      onResumeDevice={() => void resumeDeviceSession()}
      onClearEvents={() => setConnectionEvents([])}
      onOpenChange={setAppMenuOpen}
    />

    <div className="app-content">
      <main className={`workspace view-${surfaceView}`}>
        <QcDeviceDisplay display={display} hardwareGestureContextKey={JSON.stringify([connection.source, qcController.lifetime.checkpoint().session])} formFactor={formFactor} skin={skin} onAction={handleHardwareAction} onMasterVolumeInteractionStart={continuousControls.beginMasterVolumeInteraction} onMasterVolumeInteractionEnd={continuousControls.endMasterVolumeInteraction} />
      </main>
    </div>

    <div className="status-strip" role="status">
      <span className="status-symbol">i</span>
      <span className="status-notice">{visibleNotice}</span>
      <span className="status-context"><span className={`context-pill phase-${liveIndicator.phase}${liveIndicator.backgroundSync ? " is-background-sync" : ""}`} title={liveIndicator.backgroundSync ? "Live and synchronized · still receiving library data from the Quad Cortex" : undefined}>{liveIndicator.label}</span>{awaitingDeviceData && <span className="context-pill is-awaiting-data" role="status">WAITING FOR DEVICE DATA</span>}{snapshot.gridLayoutComplete === false && <span className="context-pill is-awaiting-data" role="status">{gridRefreshPending ? "REFRESHING GRID…" : "GRID INCOMPLETE"}</span>}<strong>{snapshot.presetLocation} · {snapshot.presetName}</strong><span>Scene {sceneLetter(snapshot.activeScene)}</span>{snapshot.dirty && <span className="dirty-state">UNSAVED</span>}{workspaceName && <span>{workspaceName}</span>}<span>{selectedBlockId ? snapshot.blocks.find((block) => block.id === selectedBlockId)?.name : "No selection"}</span></span>
    </div>


    {/* Copy/swap is still the Grid context-menu command. Scene label/color is
        rendered by the shared display as the current device screen, reached
        through Scene mode -> Gig View -> the scene tile's Edit control. */}
    <QcDeviceDisplayOverlay display={display} />
    {textPromptOverlay}
  </div>;
}
