import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { OwnedAttemptLatch } from "./owned-attempt-latch.ts";
import { pasteOwnedDemoLibrary } from "./demo-library-paste.ts";
import { refreshOwnedNativeSnapshot } from "./native-session-refresh.ts";
import { createBrowserPairingPoll } from "./browser-pairing-poll.ts";
import { createControlAccessChange } from "./control-access-change.ts";

test("Access-mode changes serialize before render and recover actual native access rather than optimistic rollback", async () => {
  let rejectWrite!: (error: Error) => void;
  const write = new Promise<{ accessMode: "full" }>((_resolve, reject) => { rejectWrite = reject; });
  const events: string[] = [];
  const change = createControlAccessChange({
    write: (mode) => { events.push(`write:${mode}`); return write; },
    read: async () => ({ accessMode: "full" as const }),
    observed: (mode) => events.push(`observed:${mode}`),
    uncertain: () => events.push("uncertain"),
    pending: (value) => events.push(`pending:${value}`),
    failed: () => events.push("failed")
  });
  const first = change("read-only");
  await change("modify");
  rejectWrite(new Error("write failed"));
  await first;
  assert.deepEqual(events, ["pending:true", "write:read-only", "uncertain", "observed:full", "failed", "pending:false"]);
});

test("Unconfirmed access-mode changes never invent a safe mode when recovery also fails", async () => {
  const events: string[] = [];
  const change = createControlAccessChange({
    write: async () => { throw new Error("native result unavailable"); },
    read: async () => { throw new Error("status unavailable"); },
    observed: (mode) => events.push(mode),
    uncertain: () => events.push("uncertain"),
    pending: () => undefined,
    failed: () => undefined
  });
  await change("read-only");
  assert.deepEqual(events, ["uncertain"]);
});

test("Browser pairing owns one in-flight redemption and drops a disposed result", async () => {
  let complete!: (result: { state: "paired" }) => void;
  const pending = new Promise<{ state: "paired" }>((resolve) => { complete = resolve; });
  let reads = 0;
  let paired = 0;
  const owner = createBrowserPairingPoll({
    read: () => { reads++; return pending; },
    async paired() { paired++; },
    failed() { assert.fail("Successful native pairing must not fail"); }
  });
  const work = owner.poll();
  await owner.poll();
  assert.equal(reads, 1);
  owner.dispose();
  complete({ state: "paired" });
  await work;
  assert.equal(paired, 0);
  await owner.poll();
  assert.equal(reads, 1);
  const active = createBrowserPairingPoll({ read: async () => ({ state: "paired" }), async paired() { paired++; }, failed() { assert.fail("Pairing failed"); } });
  await active.poll();
  assert.equal(paired, 1);
});

test("Native refresh discards a snapshot when the USB session or preset changed during the read", async () => {
  let current = true;
  let resolve!: (snapshot: string) => void;
  const pending = new Promise<string>((done) => { resolve = done; });
  const applied: string[] = [];
  const work = refreshOwnedNativeSnapshot({ current: () => current, read: () => pending, apply: (snapshot) => { applied.push(snapshot); } });
  current = false;
  resolve("old-hardware");
  await work;
  assert.equal(applied.length, 0);
  await refreshOwnedNativeSnapshot({ current: () => true, read: async () => "current-hardware", apply: (snapshot) => { applied.push(snapshot); } });
  assert.deepEqual(applied, ["current-hardware"]);
});

test("Demo library paste abandons work when its session changes across an asynchronous boundary", async (t) => {
  for (const boundary of ["clipboard", "confirmation", "replacement", "snapshot"] as const) {
    await t.test(boundary, async () => {
      let active = true;
      let release!: () => void;
      let entered!: () => void;
      const paused = new Promise<void>((resolve) => { release = resolve; });
      const reached = new Promise<void>((resolve) => { entered = resolve; });
      const calls: string[] = [];
      const step = async (name: typeof boundary) => {
        calls.push(name);
        if (name === boundary) { entered(); await paused; }
      };
      const work = pasteOwnedDemoLibrary({
        current: () => active,
        async readLibrary() { await step("clipboard"); return { library: "demo-only", confirmation: "Replace?" }; },
        async confirm() { await step("confirmation"); return true; },
        async replaceLibrary(library) { assert.equal(library, "demo-only"); await step("replacement"); },
        async readSnapshot() { await step("snapshot"); return "demo-snapshot"; },
        applied() { calls.push("applied"); }
      });
      await reached;
      active = false;
      release();
      await work;
      const stages = ["clipboard", "confirmation", "replacement", "snapshot"];
      assert.deepEqual(calls, stages.slice(0, stages.indexOf(boundary) + 1));
    });
  }
});

test("Demo library paste reconciles its captured virtual snapshot only after accepted replacement", async () => {
  const calls: string[] = [];
  await pasteOwnedDemoLibrary({
    current: () => true,
    async readLibrary() { return { library: "demo-only", confirmation: "Replace?" }; },
    async confirm(message) { assert.equal(message, "Replace?"); return true; },
    async replaceLibrary(library) { calls.push(`replace:${library}`); },
    async readSnapshot() { calls.push("read-demo"); return "demo-snapshot"; },
    applied(snapshot) { calls.push(`apply:${snapshot}`); }
  });
  assert.deepEqual(calls, ["replace:demo-only", "read-demo", "apply:demo-snapshot"]);
  await pasteOwnedDemoLibrary({
    current: () => true,
    async readLibrary() { return { library: "demo-only", confirmation: "Replace?" }; },
    async confirm() { return false; },
    async replaceLibrary() { assert.fail("Cancelled paste must not replace the library"); },
    async readSnapshot() { assert.fail("Cancelled paste must not read a snapshot"); },
    applied() { assert.fail("Cancelled paste must not reconcile"); }
  });
});

const generatedRelayProfileSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/GeneratedRelayProfile.java", import.meta.url), "utf8");
const completionSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcGatewayCompletion.java", import.meta.url), "utf8");
const javaSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java", import.meta.url), "utf8");
const backupFileAllocatorSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcBackupFileAllocator.java", import.meta.url), "utf8");
const backupStorageSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcBackupStorage.java", import.meta.url), "utf8");
const hidWritePolicySource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcHidWritePolicy.java", import.meta.url), "utf8");
const hidWritePolicyTestSource = readFileSync(new URL("../android/app/src/test/java/org/freevia/qcremote/QcHidWritePolicyTest.java", import.meta.url), "utf8");
const usbProfileSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcUsbProfile.java", import.meta.url), "utf8");
const usbDevicePolicySource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcUsbDevicePolicy.java", import.meta.url), "utf8");
const usbDeviceFilterSource = readFileSync(new URL("../android/app/src/main/res/xml/device_filter.xml", import.meta.url), "utf8");
const servicesSource = readFileSync(new URL("./native-services.ts", import.meta.url), "utf8");
const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const liveStateSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-live-state.ts", import.meta.url), "utf8");
const performanceWorkflowSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-performance-workflow.ts", import.meta.url), "utf8");
const coreStateSource = readFileSync(new URL("../../../packages/typescript/qc-core/src/state.ts", import.meta.url), "utf8");
const generatedPayloadSource = readFileSync(new URL("../../../packages/typescript/qc-client/src/generated-payloads.ts", import.meta.url), "utf8");
const coreFootswitchSource = readFileSync(new URL("../../../packages/typescript/qc-core/src/footswitch.ts", import.meta.url), "utf8");
const nativeDecoderSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcNativeStateDecoder.java", import.meta.url), "utf8");
const rustStateSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/state.rs", import.meta.url), "utf8");
const rustCommandsSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/commands.rs", import.meta.url), "utf8");
const rustScreenSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/screen.rs", import.meta.url), "utf8");
const presetOwnerSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/owner/preset_workflow.rs", import.meta.url), "utf8");
const presetCatalogSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/preset_catalog.rs", import.meta.url), "utf8");
const presetCoreSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/preset_workflow.rs", import.meta.url), "utf8");
const presetResultSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/owner/preset_workflow/result.rs", import.meta.url), "utf8");
const androidPresetSource = readFileSync(new URL("../../../packages/rust/quadcortex-android/src/preset_workflow.rs", import.meta.url), "utf8");
const presetSaveSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/preset_save.rs", import.meta.url), "utf8");
const rustAndroidSource = readFileSync(new URL("../../../packages/rust/quadcortex-android/src/lib.rs", import.meta.url), "utf8");
const rustRuntimeSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/mod.rs", import.meta.url), "utf8");
const rustRuntimeRequestSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/request.rs", import.meta.url), "utf8");
const rustHostMidiSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/request/host_midi.rs", import.meta.url), "utf8");
const rustStateRuntimeSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/state_runtime.rs", import.meta.url), "utf8");
const rustTransportRuntimeSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/transport.rs", import.meta.url), "utf8");
const rustSessionSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/session.rs", import.meta.url), "utf8");
const rustInitializationSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/initialization.rs", import.meta.url), "utf8");
const rustBackupRuntimeSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/backup.rs", import.meta.url), "utf8");
const rustCatalogRuntimeSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/device/catalog.rs", import.meta.url), "utf8");
const rustResponseSource = readFileSync(new URL("../../../packages/rust/quadcortex/src/responses.rs", import.meta.url), "utf8");
const deviceBrokerRpcSource = readFileSync(new URL("../../../services/device-broker/src/rpc.rs", import.meta.url), "utf8");
const sharedTransportSource = readFileSync(new URL("../../../packages/typescript/qc-core/src/gateway-transport.ts", import.meta.url), "utf8");
const continuousControlSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-continuous-control-workflow.ts", import.meta.url), "utf8");
const corOsGigViewSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/coros-gig-view.tsx", import.meta.url), "utf8");
const generatedGatewaySource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/GeneratedGatewayMethods.java", import.meta.url), "utf8");
const remoteActionsSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/GeneratedRemoteActions.java", import.meta.url), "utf8");
const relayProtocolSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayProtocol.java", import.meta.url), "utf8");
const relayServiceSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayService.java", import.meta.url), "utf8");
const relayFrameHandlerSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayFrameHandler.java", import.meta.url), "utf8");
const relayExceptionSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayException.java", import.meta.url), "utf8");
const relayPluginSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayPlugin.java", import.meta.url), "utf8");
const relayCredentialStoreSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayCredentialStore.java", import.meta.url), "utf8");
const actionContract = JSON.parse(readFileSync(new URL("../../../contracts/public/qc-actions.v1.json", import.meta.url), "utf8"));
const gatewayContract = JSON.parse(readFileSync(new URL("../../../contracts/public/gateway-methods.v1.json", import.meta.url), "utf8"));
const androidManifestSource = readFileSync(new URL("../android/app/src/main/AndroidManifest.xml", import.meta.url), "utf8");
const capacitorConfigSource = readFileSync(new URL("../capacitor.config.json", import.meta.url), "utf8");
const splashFallbackSource = readFileSync(new URL("../android/app/src/main/res/drawable/splash.xml", import.meta.url), "utf8");

test("tempo synchronizes in both directions over the native USB bridge", () => {
  assert.match(sharedTransportSource, /gateway\.setTempo\(bpm, state\.tempo, state\.presetName, requirePresetIdentity\(state\)\)/);
  assert.match(generatedPayloadSource, /\| "tempo"/);
  assert.match(
    javaSource,
    /writeMessages\(plan\.messages[\s\S]{0,350}stateDecoder\.gatewayVerificationStarted\([\s\S]{0,120}operation\.afterSequence, monotonicMillis\(\)/,
  );
  assert.match(
    rustRuntimeRequestSource,
    /preset_transition[\s\S]{0,300}profile::PRESET_SYNC_TIMEOUT_MS[\s\S]{0,100}profile::COMMAND_CONFIRMATION_TIMEOUT_MS/,
  );
  assert.match(rustRuntimeRequestSource, /DeviceCommand::SetTempo\(bpm\)/);
  assert.match(rustCommandsSource, /pub fn set_tempo/);
  assert.match(rustStateSource, /decode_global_tempo/);
  assert.match(rustStateSource, /StateUpdate::new\("tempo"\)/);
  assert.match(rustStateSource, /tempo_led_enabled/);
  assert.match(appSource, /consumeQcNativeStateFrame\(frame/);
  assert.match(liveStateSource, /reconcileFrame\(states, observedAt\)/);
  assert.match(performanceWorkflowSource, /transport\.tapTempo\(controller\.snapshotRef\.current\)/);
});

test("USB attachment connects automatically and reports synchronization separately", () => {
  assert.match(servicesSource, /connected: boolean; synchronized: boolean/);
  assert.match(javaSource, /isReady\(\) && stateDecoder\.sessionSynchronized\(\) && currentSetlist != null/);
  assert.match(appSource, /if \(state === "available"\)[\s\S]{0,120}attemptUsbConnection\(\)/);
  assert.match(
    appSource,
    /if \(!devices\.length\) \{\s*setHardwareConnected\(false\);\s*if \(activeSourceRef\.current !== "virtual"\) \{\s*transitionConnection\("absent"\);\s*void startDemoDeviceRef\.current\("No Quad Cortex was found\."\);/,
    "an empty scan falls back to Demo only when hardware was the active source",
  );
  assert.match(appSource, /state\.kind === "preset"[\s\S]*usbSessionReady\.current[\s\S]*transitionConnection\("connected"\)/);
  assert.match(appSource, /const \[hardwareConnected, setHardwareConnected\] = useState\(false\)/, "native USB readiness remains independent while the demo is active");
  assert.match(appSource, /qcConnectionIndicatorPhase\(connection, hardwareConnected && !demoActive, null, \{ actionRequired: usbWritesQuarantined \}\)/, "the device pill uses the global indicator contract, including write quarantine state");
  assert.doesNotMatch(appSource, /hardwareConnected \? "ready" : usbIndicatorPhase/, "a merely open USB handle cannot force any active-device status dot green");
  assert.match(appSource, /usbIndicatorPhase === "ready"[\s\S]{0,320}"Quad Cortex disconnected"/, "the USB panel words and colors share the same readiness phase");
  assert.match(appSource, /demoActive \? "DEMO" : "USB"/, "the compact pill names the active device source");
  assert.doesNotMatch(appSource, /qcReadyLabel\(connection\)/, "Android does not repeat the verbose desktop readiness label");
});

test("reset session tears down the Android epoch and waits for a new authoritative seed", () => {
  assert.match(generatedGatewaySource, /case "device\.resetSession": return "RECONNECT"/);
  assert.match(javaSource, /"device\.resetSession"\.equals\(method\) \? "Communication session reset"/);
  assert.match(javaSource, /if \(reconnectInFlight != null\) return reconnectInFlight/);
  assert.match(javaSource, /openDeviceAndHandshake\(candidate, pending, relayAdmissionGuard\.epoch\(\)\)/);
  assert.match(javaSource, /private void openDeviceAndHandshake[\s\S]*synchronized \(this\)[\s\S]*claimBackupRecovery\(ready\.backupOrigin\)[\s\S]*closeConnection\(preservedOperation\)/);
  assert.match(javaSource, /closeConnection\(QcPendingOperations\.Entry<\?> preservedOperation\)\s*\{\s*closeConnection\(preservedOperation, true\);\s*\}/);
  assert.match(javaSource, /closeConnection\(\s*QcPendingOperations\.Entry<\?> preservedOperation, boolean sendDisconnectCommands\s*\)\s*\{[\s\S]*?connectionGeneration\.incrementAndGet\(\);\s*UsbRequest\[\] inputRequests/);
  assert.match(javaSource, /pendingOperations\.failAllExcept\(preservedOperation/);
  assert.match(javaSource, /stateDecoder\.sessionDisconnected\(monotonicMillis\(\)\)[\s\S]{0,80}stateDecoder\.reset\(\)/);
  assert.match(javaSource, /stateDecoder\.sessionOpened\(monotonicMillis\(\)\)[\s\S]{0,80}startReader\(\)/);
  assert.match(javaSource, /resolvePendingReady[\s\S]{0,250}!stateDecoder\.sessionSynchronized\(\) \|\| currentSetlist == null/);
  assert.match(javaSource, /pendingOperations\.timeout\(pending, QcUsbProfile\.READY_WAIT_TIMEOUT_MS/);
});

test("state event cursors remain monotonic across Android disconnects", () => {
  assert.match(javaSource, /private long nextStateSequence = 1;/);
  assert.match(javaSource, /stateEventLog\.clear\(\);[\s\S]{0,80}latestTempoClock = null;/);
  assert.doesNotMatch(javaSource, /stateEventLog\.clear\(\);[\s\S]{0,80}nextStateSequence = 1;/);
  assert.match(javaSource, /if \(after < 0\)[\s\S]{0,180}if \(limit < 1 \|\| limit > QcDomain\.STATE_EVENT_MAXIMUM_LIMIT\)/);
});

test("A through H use the reported hardware mode and assignments", () => {
  assert.match(sharedTransportSource, /gateway\.pressFootswitch\(index, state\.mode, state\.presetName, requirePresetIdentity\(state\)\)/);
  assert.match(rustStateSource, /preset[\s\S]{0,200}\.stomp_mode_assignments/);
  assert.match(rustStateSource, /footswitch: assignments\.get/);
  assert.match(rustStateSource, /footswitch_assignment_type/);
  assert.match(rustStateSource, /stomp_assignment_type\(item\)/);
  assert.match(generatedPayloadSource, /footswitchAssignmentType\?: "primary" \| "secondary"/);
  assert.match(performanceWorkflowSource, /controller\.beginFootswitch/);
  assert.match(coreFootswitchSource, /mode === "SCENE"/);
  assert.match(coreFootswitchSource, /mode === "PRESET"/);
  assert.match(coreFootswitchSource, /block\.footswitch === index/);
  assert.match(rustHostMidiSource, /"device\.pressFootswitch"[\s\S]*profile::FOOTSWITCH_BASE_CONTROLLER/);
  assert.match(performanceWorkflowSource, /controller\.runFootswitch\(transport, index\)/);
});

test("mode slots A through C use the shared native HID route", () => {
  assert.match(sharedTransportSource, /gateway\.selectModeSlot\(slot, state\.presetName, requirePresetIdentity\(state\)\)/);
  assert.match(rustRuntimeRequestSource, /"device\.selectModeSlot"[\s\S]*configured_mode_for_slot\(snapshot, slot\)/);
  assert.match(rustRuntimeRequestSource, /HidCommand\(DeviceCommand::SelectMode\(mode\)\)/);
  assert.match(performanceWorkflowSource, /controller\.runModeSlot\(transport, slot\)/);
});

test("Tap Tempo uses its explicit official MIDI control and live bypass updates are batched", () => {
  assert.match(sharedTransportSource, /gateway\.tapTempo\(state\.mode, state\.presetName, requirePresetIdentity\(state\)\)/);
  assert.match(rustHostMidiSource, /"device\.tapTempo"[\s\S]*profile::TAP_TEMPO_CONTROLLER/);
  assert.match(rustRuntimeRequestSource, /MidiControlChange/);
  assert.match(performanceWorkflowSource, /transport\.tapTempo\(controller\.snapshotRef\.current\)/);
  assert.match(rustStateSource, /StateUpdate::new\("bypassBatch"\)/);
  assert.match(coreStateSource, /state\.kind === "bypassBatch"/);
});

test("native USB remains open and command traffic is never blocked by startup", () => {
  assert.match(javaSource, /readerIo = Executors\.newSingleThreadExecutor\(\)/);
  assert.match(javaSource, /commandIo = Executors\.newSingleThreadExecutor\(\)/);
  assert.match(javaSource, /midiIo = Executors\.newSingleThreadExecutor\(\)/);
  assert.match(javaSource, /private CompletableFuture<org\.json\.JSONObject> relayMidi/);
  assert.match(javaSource, /if \(isReady\(\) && device != null\s*&& device\.getDeviceId\(\) == candidate\.getDeviceId\(\)\)/);
  assert.doesNotMatch(javaSource, /Thread\.sleep\(2000\)/);
  assert.match(javaSource, /submitRelayTask\(midiIo, relayDeadline, \(\) ->/);
  assert.match(javaSource, /midiConnection = publishMidiConnection/);
  assert.match(javaSource, /activeMidiConnection\.bulkTransfer\(midiOutputEndpoint/);
  assert.match(javaSource, /hostStartedAtUnixMs/);
  assert.match(javaSource, /dispatchLatencyMs/);
  assert.match(javaSource, /midiConnection\.releaseInterface\(midiInterface\)/);
  assert.match(appSource, /consumeQcNativeStateFrame\(frame/);
  assert.match(liveStateSource, /reconcileFrame\(states, observedAt\)/);
  assert.match(readFileSync(new URL("../../../packages/typescript/qc-core/src/command-coordinator.ts", import.meta.url), "utf8"), /beginSnapshotMutation/);
  assert.match(rustStateSource, /catalog_refresh = Some\(true\)/);
});

test("one device frame crosses the bridge once with all realtime state updates", () => {
  assert.match(rustAndroidSource, /owner\.observe_state\(&ticket, frame\.message_type, &frame\.payload\)/);
  assert.match(rustStateSource, /fn decode_grid/);
  assert.match(javaSource, /notifyListeners\("qcStateBatch", frame, false\)/);
  assert.match(appSource, /useNativeStateRecovery\(/);
  assert.match(appSource, /androidGatewayTransport\.currentStateEvents\(after, limit\)/);
  assert.doesNotMatch(javaSource, /notifyListeners\("qcState", state/);
  assert.match(servicesSource, /addListener\(eventName: "qcStateBatch"/);
  assert.match(appSource, /QcUsbNative\.addListener\("qcStateBatch"/);
  assert.match(appSource, /consumeQcNativeStateFrame\(frame/);
  assert.match(coreStateSource, /reduceQcStateFrame/);
  assert.match(javaSource, /state\.put\("observedAt", observedAt\)/);
});

test("large model metadata never blocks the permanent USB reader", () => {
  assert.match(javaSource, /metadataIo = new java\.util\.concurrent\.ThreadPoolExecutor\([\s\S]{0,100}ArrayBlockingQueue<>\(1\)/);
  assert.match(javaSource, /if \(type == QcUsbProfile\.MESSAGE_TYPE_MODEL_REPO\) \{[\s\S]*scheduleModelCatalogDecode/);
  assert.match(javaSource, /metadataIo\.execute\(\(\) ->/);
  assert.match(javaSource, /stateDecoder\.queueModelRepo\(observationTicket, payload\)/);
  assert.match(rustAndroidSource, /parse_model_repo\(&work\.payload\)/);
  assert.doesNotMatch(javaSource, /Base64\.encodeToString|notifyListeners\("qcUsbMessage"/, "raw frames must not be serialized across the bridge");
  assert.doesNotMatch(servicesSource, /addListener\(eventName: "qcUsbMessage"/);
  assert.match(appSource, /Promise\.all\(listenerPromises\)[\s\S]*QcUsbNative\.scan\(\)/, "listeners must be live before startup frames arrive");
  assert.match(javaSource, /stateDecoder\.lifecycleNextBatch\(/);
  assert.doesNotMatch(javaSource, /stateDecoder\.(?:systemTimeCommand|postBootInitializationStarted|startupBeginBuilding)\(/);
  assert.match(javaSource, /lifecycleDispatched\(batch, false, monotonicMillis\(\)\)/);
  assert.match(rustSessionSource, /DispatchOutcome::Failed[\s\S]{0,120}self\.fail\(SessionFailure::Dispatch\)/);
  assert.match(javaSource, /if \(connection != null\) terminalTransportFailed\(lastError, "lifecycle-write-failed"\)/);
  assert.match(javaSource, /stateDecoder\.lifecyclePoll\(monotonicMillis\(\)\)/);
  assert.match(rustAndroidSource, /frame\.message_type == quadcortex::profile::MESSAGE_TYPE_MODEL_REPO/);
  assert.doesNotMatch(javaSource, /private volatile boolean startupActive/);
  assert.match(rustInitializationSource, /deadline_ms: now_ms\.saturating_add\(profile::READY_WAIT_TIMEOUT_MS\)/);
  assert.match(javaSource, /stateDecoder\.sessionSynchronized\(\) && currentSetlist != null/);
  assert.match(javaSource, /connection == null \|\| !stateDecoder\.lifecycleBatchCurrent\(batch\)/);
  assert.match(javaSource, /QcUsbProfile\.POST_INITIALIZATION_WRITE_DELAY_MS/);
  assert.match(rustInitializationSource, /self\.synchronized && self\.seed_complete\(\)/);
  assert.match(rustCommandsSource, /pub fn subscribe_cpu_load/);
});

test("the Capacitor bridge delegates commands, framing, and state to shared Rust", () => {
  assert.match(javaSource, /stateDecoder\.pushReport\(nativeConnection, reportEpoch, report, receivedAtMs\)/);
  assert.match(javaSource, /stateDecoder\.captureReportEpoch\(nativeConnection\)/);
  assert.doesNotMatch(javaSource, /reportFlags|MAX_FRAME_REPORTS|List<byte\[]> reports/);
  assert.match(javaSource, /stateDecoder\.encodeReports\(message, withReportId, dispatchTicket, monotonicMillis\(\)\)/);
  assert.doesNotMatch(javaSource, /Arrays\.copyOfRange\(framedReport, 1, framedReport\.length\)/);
  assert.match(javaSource, /stateDecoder\.gatewayPlan\(method, JSObject\.fromJSONObject\(params\)\)/);
  assert.match(rustRuntimeRequestSource, /pub fn gateway_write_verification_policy/);
  assert.match(rustRuntimeRequestSource, /pub struct GatewayVerificationRuntime/);
  assert.match(rustAndroidSource, /gateway_write_verification_policy\(&method\)/);
  assert.match(nativeDecoderSource, /gatewayVerificationStarted/);
  assert.match(nativeDecoderSource, /gatewayVerificationAdvance/);
  assert.match(javaSource, /stateDecoder\.gatewayVerificationAdvance\(/);
  assert.doesNotMatch(javaSource, /for \(long refreshDelay : plan\.refreshDelaysMs\)/);
  assert.doesNotMatch(javaSource, /writeMessage\(message, !includeReportId\)/);
  assert.doesNotMatch(javaSource, /isIdempotentGatewayWrite/);
  assert.doesNotMatch(javaSource, /QcUsbFraming|QcProtobufWire|fieldVarint|fieldMessage/);
  assert.match(nativeDecoderSource, /System\.loadLibrary\("quadcortex_android"\)/);
  assert.match(nativeDecoderSource, /nativeEncodeCommand/);
  assert.doesNotMatch(nativeDecoderSource, /readCommand\(int messageType\)/);
  assert.doesNotMatch(rustAndroidSource, /"read"\s*=>/);
  assert.match(nativeDecoderSource, /nativePlanGatewayWrite/);
  assert.match(nativeDecoderSource, /plan = new JSONObject\(nativePlanGatewayWrite/);
  assert.match(nativeDecoderSource, /private static native String nativePlanGatewayRead/);
  assert.doesNotMatch(nativeDecoderSource, /littleEndian(?:Int|Long)|decodeCommandEnvelope|DecodedEnvelope/);
  assert.match(rustAndroidSource, /fn messages_json/);
  assert.doesNotMatch(rustAndroidSource, /fn message_envelope/);
  assert.match(nativeDecoderSource, /nativeEncodeReports/);
  assert.match(nativeDecoderSource, /nativePushReport/);
  assert.match(rustAndroidSource, /quadcortex::state/);
  assert.match(rustAndroidSource, /quadcortex::commands/);
  assert.match(rustAndroidSource, /quadcortex::framing/);
  assert.match(rustAndroidSource, /quadcortex::device::transport::\{ReportLayout, TransportRuntime\}/);
  assert.match(rustTransportRuntimeSource, /FrameAssembler/);
  assert.match(rustTransportRuntimeSource, /normalize_inbound_report/);
  assert.doesNotMatch(rustAndroidSource, /quadcortex::session::(?:FrameAssembler|\{[^}]*FrameAssembler)/);
});

test("Android feeds decoded state into the shared native device runtime", () => {
  assert.match(rustAndroidSource, /state_runtime::DeviceStateRuntime/);
  assert.match(rustStateRuntimeSource, /pub struct DeviceStateRuntime/);
  assert.match(rustStateRuntimeSource, /self\.snapshot\.apply\(state\)/);
  assert.match(rustAndroidSource, /nativeSnapshot/);
  assert.match(rustAndroidSource, /plan_gateway_write/);
  assert.match(nativeDecoderSource, /JSObject snapshot\(\).*nativeSnapshot/);
});

test("Android generic dispatch covers the generated Grid, routing, and MIDI actions", () => {
  for (const method of ["device.moveBlock", "device.addBlock", "device.removeBlock", "device.setBlockFootswitch", "device.setChainInput", "device.setChainOutput", "device.setChainSplit"]) {
    assert.match(rustRuntimeRequestSource, new RegExp(method.replace(".", "\\.")));
  }
  assert.match(generatedGatewaySource, /case "device\.pressFootswitch": return "PLANNED_WRITE"/);
  assert.match(generatedGatewaySource, /case "device\.tapTempo": return "PLANNED_WRITE"/);
  assert.match(generatedGatewaySource, /case "device\.selectModeSlot": return "PLANNED_WRITE"/);
  assert.match(javaSource, /stateDecoder\.gatewayPlan\(method, JSObject\.fromJSONObject\(params\)\)/);
  assert.match(nativeDecoderSource, /PlannedGatewayWrite/);
});

test("Android remote relay consumes every generated MCP action with verified writes", () => {
  for (const action of actionContract.actions) {
    assert.match(remoteActionsSource, new RegExp(`"${action.rpc.replace(".", "\\.")}"`), `${action.rpc} is absent from Android remote policy`);
  }
  assert.match(relayProtocolSource, /GeneratedRemoteActions\.contains\(method\)/);
  // The legacy raw/command broker surface was deleted. No
  // remote policy, gateway method or broker dispatch may name it again.
  for (const pattern of [/device\.raw\./, /device\.command\./]) {
    assert.doesNotMatch(remoteActionsSource, pattern, `${pattern} must not reach the remote relay`);
    assert.doesNotMatch(generatedGatewaySource, pattern, `${pattern} is not a gateway method`);
  }
  assert.doesNotMatch(deviceBrokerRpcSource, /request\.method == "device\.(raw|command)\./, "the broker must not dispatch the deleted legacy surface");
  assert.match(javaSource, /relayPlannedGatewayWrite\(/);
  assert.match(javaSource, /stateDecoder\.gatewayVerificationAdvance\(/);
  assert.match(javaSource, /resolvePendingGatewayTransactions/);
  assert.doesNotMatch(javaSource, /pollRelayVerification/);
  assert.match(javaSource, /CURRENT_RELAY_DEADLINE\.set\(relayDeadline\)[\s\S]{0,120}relayGatewayWorkflow\(method, params\)/);
  assert.match(nativeDecoderSource, /nativeBeginPresetWorkflow/);
  assert.match(nativeDecoderSource, /nativeAdvancePresetWorkflow/);
  assert.match(javaSource, /stateDecoder\.beginPresetWorkflow/);
  assert.match(javaSource, /stateDecoder\.advancePresetWorkflow/);
  assert.match(javaSource, /writeMessages\(step.messages, interval, operation\)/);
  assert.match(androidPresetSource, /begin_preset_preparation/);
  assert.match(androidPresetSource, /advance_preset_workflow/);
  assert.match(androidPresetSource, /preset_workflow_result/);
  assert.match(presetCatalogSource, /reconcile_saved_preset_records/);
  assert.match(presetSaveSource, /stored_preset_name_matches/);
  assert.match(presetResultSource, /self.outcome.catalog_verified && self.catalog.is_some\(\)/);
  assert.match(presetResultSource, /value\["savedName"\] = json!\(saved_name\)/);

  assert.match(usbProfileSource, /PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS = 45000L/);
  assert.match(usbProfileSource, /PRESET_LIBRARY_VERIFICATION_RETRY_MS = 500L/);
  assert.match(rustCatalogRuntimeSource, /pub struct CatalogVerificationRuntime/);
  assert.match(rustCatalogRuntimeSource, /profile::PRESET_LIBRARY_VERIFICATION_TIMEOUT_MS/);
  assert.match(rustCatalogRuntimeSource, /profile::PRESET_LIBRARY_VERIFICATION_RETRY_MS/);
  assert.match(rustCatalogRuntimeSource, /profile::PRESET_LIBRARY_REFRESH_COALESCE_MS/);
  assert.doesNotMatch(javaSource, /shouldRecoverAfterSilence|USB session recovered after silent/);
  assert.doesNotMatch(javaSource, /USB session refreshed for duplicate-setlist verification/);
  assert.doesNotMatch(nativeDecoderSource, /nativeCatalogVerificationStarted|nativeCatalogListingObserved/);
  assert.doesNotMatch(rustAndroidSource, /catalog_verifications: Mutex|nativeCatalogVerificationAdvance/);
  assert.match(presetOwnerSource, /catalog.runtime.take_action\(now_ms\)/);
  assert.match(presetCatalogSource, /generation_observed/);
  assert.doesNotMatch(javaSource, /nextRetryDelayMs/);
  assert.match(javaSource, /persistentMutationGate\.submit\(\(\) -> \{/);
  assert.match(javaSource, /CURRENT_RELAY_DEADLINE\.set\(relayDeadline\)[\s\S]*relayGatewayWorkflow\(method, params\)/);
  assert.doesNotMatch(javaSource, /USB session refreshed for saved preset verification/);
  assert.match(androidPresetSource, /gateway_read_scope_remaining\(&scope, now_ms\)/);
  assert.match(generatedGatewaySource, /case "device\.deletePreset": return "PERSISTENT_WRITE"/);
  assert.doesNotMatch(javaSource, /relaySetDeviceName[\s\S]{0,1800}authoritative_readback/);
  assert.doesNotMatch(javaSource, /Character\.isDigit/);
  assert.match(rustRuntimeRequestSource, /pub fn stored_preset_name_matches/);
  assert.match(relayServiceSource, /"MALFORMED_RESPONSE"/);
  assert.doesNotMatch(
    relayServiceSource,
    /whenComplete\(\(result, error\)[\s\S]{0,1600}catch \(Exception ignored\) \{\}/,
    "relay completion errors must produce an explicit response instead of becoming host timeouts",
  );
  assert.match(rustRuntimeRequestSource, /pub fn plan_preset_mutation/);
  assert.doesNotMatch(javaSource, /relaySavePresetAs|relayRenamePreset|relayCopyPreset|relayGridVerification/);
  assert.match(javaSource, /stateDecoder\.modelList\(\)/);
  assert.match(javaSource, /relayCatalogRead\(method, params\)/);
  assert.match(rustStateRuntimeSource, /self\.native_presets\.apply\(&message\)/);
  assert.match(rustAndroidSource, /nativeRetainedCatalog/);
  for (const method of ["device.identity", "device.inhibitedModules", "device.presetScreenshot", "device.captureScreen"]) {
    assert.match(rustRuntimeRequestSource, new RegExp(method.replace(".", "\\.")));
  }
  assert.match(
    javaSource,
    /case "CORRELATED_READ":[\s\S]{0,250}relayGatewayRead\(method, params\)/,
  );
  assert.match(javaSource, /relaySetDeviceName\(params\)/);
  assert.match(javaSource, /relayScreenGesture\(method, params\)/);
  assert.match(nativeDecoderSource, /nativePlanGatewayRead/);
  assert.match(nativeDecoderSource, /nativeObserveGatewayRead/);
  assert.match(javaSource, /readResponseIo\.execute\(\(\) -> completeGatewayRead/);
  assert.match(rustResponseSource, /decode_device_identity/);
  assert.match(rustResponseSource, /decode_inhibited_modules/);
  assert.match(rustResponseSource, /decode_preset_screenshot/);
  assert.match(rustResponseSource, /decode_captured_screen/);
  assert.doesNotMatch(javaSource, /VersionMessage|ScreenshotMessage|CompilerInhibitedModulesMessage|RemoteControlMessage/);
});

test("Android keeps exact local gateway parity while its remote surface honors explicit exclusions", () => {
  const remote = new Set(["system.status", ...actionContract.actions.map((action: { rpc: string }) => action.rpc)]);
  const gateway = gatewayContract.methods.map((method: { rpc: string }) => method.rpc);
  assert.deepEqual(
    gateway.filter((method: string) => !remote.has(method)).sort(),
    [...actionContract.localOnlyGatewayRpcs].sort(),
  );
  for (const method of remote) {
    assert.match(remoteActionsSource, new RegExp(`"${method.replace(".", "\\.")}"`));
  }
  for (const method of gateway) {
    assert.match(generatedGatewaySource, new RegExp(`"${method.replace(".", "\\.")}"`));
  }
  for (const implementation of [
    "relayReconnect", "relayDisconnect", "relayStateEvents", "relayTempoClock",
    "relayCreateBackup"
  ]) assert.match(javaSource, new RegExp(implementation.replace(".", "\\.")));
  assert.match(javaSource, /relayPreviewParameter\(method, params\)/);
  assert.match(nativeDecoderSource, /header\.getJSONObject\("tempoClock"\)/);
  assert.match(nativeDecoderSource, /nativeConsumeBackupChunk/);
  assert.match(rustAndroidSource, /observation\.and_then\(\|value\| value\.tempo_clock\)/);
  assert.match(rustAndroidSource, /owner\.consume_backup/);
  assert.match(rustBackupRuntimeSource, /BackupAssembler/);
  assert.match(javaSource, /public void gatewayInvoke\(PluginCall call\)/);
  assert.match(servicesSource, /gatewayInvoke<T>\(options:/);
});

test("Android persists backups and requires a fresh synchronized USB session before completion", () => {
  assert.match(backupStorageSource, /MediaStore\.Downloads\.EXTERNAL_CONTENT_URI/);
  assert.match(backupStorageSource, /R\.string\.download_folder/);
  assert.match(javaSource, /backupFileIo\.execute\(\(\) -> \{[\s\S]*saveBackupDocument/);
  assert.match(backupStorageSource, /QcUsbProfile\.BACKUP_MAXIMUM_DOCUMENT_BYTES/);
  assert.match(usbProfileSource, /BACKUP_MAXIMUM_DOCUMENT_BYTES = 33554432/);
  assert.doesNotMatch(javaSource, /pending\.result\.complete\(JSObject\.fromJSONObject\(\(org\.json\.JSONObject\) update\.get\("backup"\)\)\)/);
  assert.match(javaSource, /recoverSessionAfterBackup\(pending\)/);
  assert.match(javaSource, /pendingOperations\.remove\(pending\)[\s\S]*pendingBackup = null;[\s\S]*relayReconnect\("USB session restored after device backup", pending\.operation\.admission\)/);
  assert.match(javaSource, /relayReconnect\("USB session restored after device backup", pending\.operation\.admission\)[\s\S]*pending\.result\.complete\(savedResult\)/);
  assert.match(javaSource, /The backup was saved, but the Quad Cortex session did not recover/);
  assert.match(javaSource, /backupOrigin == null \|\| pending\.operation\.openingStarted/);
  assert.match(javaSource, /pending\.result\.isDone\(\) \|\| pendingReady != pending/);
  assert.match(rustAndroidSource, /owner\.claim_backup_recovery\(&operation\)/);
  assert.doesNotMatch(javaSource, /completePendingBackupRecovery|recoveryAfterState/);
  for (const method of ["device.captureScreen", "device.presetScreenshot", "device.captures", "device.irs"])
    assert.match(generatedGatewaySource, new RegExp(method.replace(".", "\\.")));
  assert.doesNotMatch(javaSource, /USB session refreshed for high-volume read/);
});

test("Android legacy backup exports reserve a new filename and preserve existing backups", () => {
  assert.match(javaSource, /return QcBackupStorage\.saveDocument\(getContext\(\), document, requestedName\)/);
  assert.match(backupStorageSource, /File file = QcBackupFileAllocator\.createUniqueFile\(root, fileName\)/);
  assert.match(backupStorageSource, /exportedName = file\.getName\(\)/);
  assert.match(backupStorageSource, /MediaStore\.Downloads\.DISPLAY_NAME, fileName/);
  assert.match(backupStorageSource, /\.put\("name", exportedName\)/);
  assert.match(backupStorageSource, /new FileOutputStream\(file\)/);
  assert.match(backupStorageSource, /catch \(Exception error\)[\s\S]*file\.delete\(\)[\s\S]*throw error/);
  assert.match(backupStorageSource, /Build\.VERSION\.SDK_INT >= Build\.VERSION_CODES\.Q[\s\S]*else \{[\s\S]*QcBackupFileAllocator\.createUniqueFile/);
  assert.match(backupStorageSource, /getExternalFilesDir\(Environment\.DIRECTORY_DOWNLOADS\)[\s\S]{0,200}getFilesDir\(\)/);
  assert.match(backupStorageSource, /result\.put\("storageNotice", legacyNotice\(\)\)/);
  assert.match(appSource, /Android 7–9, native backups use app-private storage and are removed if QC Remote is uninstalled/);
  assert.match(backupFileAllocatorSource, /static File createUniqueFile\(File root, String fileName\)[\s\S]*candidate\.createNewFile\(\)/);
  assert.match(backupFileAllocatorSource, /stem \+ " \(" \+ suffix \+ "\)" \+ extension/);
});

test("Android 16 starts the connected-device relay only after the USB permission prerequisite", () => {
  assert.match(androidManifestSource, /android\.permission\.FOREGROUND_SERVICE_CONNECTED_DEVICE/);
  assert.doesNotMatch(androidManifestSource, /android\.permission\.CHANGE_NETWORK_STATE/);
  assert.match(androidManifestSource, /android:foregroundServiceType="connectedDevice"/);
  assert.match(javaSource, /static boolean relayForegroundServiceEligible\(\)/);
  assert.match(javaSource, /manager\.hasPermission\(candidate\)/);
  assert.match(relayServiceSource, /if \(!QcUsbPlugin\.relayForegroundServiceEligible\(\)\)[\s\S]{0,300}stopSelf\(\)/);
  assert.match(relayPluginSource, /USB_PERMISSION_REQUIRED/);
  assert.match(appSource, /usePublicRelayWorkflow\(\{[^}]*autoStart: hardwareConnected/);
});

test("Android discovers every product in the generated shared USB profile", () => {
  assert.match(usbProfileSource, /SUPPORTED_PRODUCT_IDS = \{34826, 35119\}/);
  assert.match(usbProfileSource, /static boolean supportsProductId\(int productId\)/);
  assert.match(usbDevicePolicySource, /matches\(candidate\.getVendorId\(\), candidate\.getProductId\(\)\)/);
  assert.match(javaSource, /QcUsbDevicePolicy\.isQuadCortex\(candidate\)/);
  assert.match(usbDeviceFilterSource, /vendor-id="5418" product-id="34826"/);
  assert.match(usbDeviceFilterSource, /vendor-id="5418" product-id="35119"/);
});

test("a stale USB completion releases its own latch after Demo fallback and cannot release a later attach", () => {
  const latch = new OwnedAttemptLatch();
  const failedUsbAttempt = latch.acquire();
  assert.notEqual(failedUsbAttempt, undefined);
  assert.equal(latch.isHeld, true);

  // Demo fallback starts a new connection epoch, but the USB attempt still
  // owns this latch until its finally block runs.
  assert.equal(latch.release(failedUsbAttempt!), true);
  assert.equal(latch.isHeld, false, "the next USB attach is no longer suppressed");

  const laterAttach = latch.acquire();
  assert.notEqual(laterAttach, undefined);
  assert.equal(latch.release(failedUsbAttempt!), false, "a delayed old finally cannot clear the new attempt");
  assert.equal(latch.isHeld, true);
  assert.match(appSource, /finally\s*\{\s*const released = connectAttemptLatch\.current\.release\(latchOwner\);/);
  assert.match(appSource, /if \(released && usbRetryRequested\.current && !usbSessionReady\.current/);
  assert.match(appSource, /connectionAttempts\.invalidate\(\);\s*connectAttemptLatch\.current\.clear\(\);/);
});

test("Android relay retains the server device ID required by the cloud socket route", () => {
  assert.match(relayPluginSource, /payload\.getString\("deviceId"\)/);
  assert.match(relayCredentialStoreSource, /private static final String DEVICE_ID = "device_id"/);
  assert.match(relayCredentialStoreSource, /save\(String endpoint, String credential, String deviceId, String ownerEmail\)/);
  assert.match(relayCredentialStoreSource, /boolean paired\(\) \{ return endpoint\(\) != null && deviceId\(\) != null/);
  assert.match(relayServiceSource, /try \{ held = store\.snapshot\(\); \}[\s\S]*String deviceId=held\.deviceId;/);
  // The id is encoded into the one query value (RelaySecurityPolicyTest
  // exercises a hostile id), and the socket is built only from a permitted endpoint.
  assert.match(relayServiceSource, /DEVICE_CONNECT_PATH \+ "\?deviceId=" \+ encodedId/);
  assert.match(relayServiceSource, /String wsEndpoint = relaySocketUrl\(permitted, deviceId\)/);
  assert.doesNotMatch(relayServiceSource, /"\?deviceId=" \+ deviceId/);
  assert.match(relayPluginSource, /String deviceId = store\.deviceId\(\); if \(deviceId != null\) result\.put\("deviceId", deviceId\)/);
});

test("Android pairing applies shared owner policy during guarded credential adoption", () => {
  // The relay mints its offer for whichever Google account signs in, and this
  // phone stored whatever came back. A pairing link that reached the wrong
  // person was enough to move the phone onto their account.
  assert.match(relayCredentialStoreSource, /private static final String OWNER_EMAIL = "owner_email"/);
  assert.match(relayCredentialStoreSource, /String ownerEmail\(\) \{ return prefs\(\)\.getString\(OWNER_EMAIL, ""\)/);
  // The decision itself is exercised in RelaySecurityPolicyTest; here, that the
  // pairing path consults it with whether a pairing already exists, so an
  // empty legacy account is refused rather than treated as a match.
  assert.match(relayPluginSource, /store\.adoptIfCurrent\(heldSnapshot, endpoint, credential, deviceId, arriving\)/);
  assert.match(relayCredentialStoreSource, /String adoptIfCurrent\([^\n]*\) throws Exception \{\s*synchronized \(MUTATION_LOCK\) \{\s*if \(hasPendingRevocation\(\)\) return "PAIRING_REVOCATION_PENDING";\s*Snapshot held=snapshot\(\);\s*if \(!sameIdentity\(held,expected\)\) return "PAIRING_SUPERSEDED";\s*if \(hasUnacknowledgedReplacement\(\)\) return "PAIRING_REPLACEMENT_PENDING";\s*String refusal=GeneratedRelayProfile\.pairingOwnerRefusal\(held\.paired\(\),held\.owner,owner,held\.rejected\);\s*if \(refusal != null\) return refusal;\s*save\(endpoint,credential,deviceId,owner\);/);
  assert.match(generatedRelayProfileSource, /PAIRING_OWNER_CHANGED/);
  assert.match(generatedRelayProfileSource, /PAIRING_OWNER_UNKNOWN/);
  assert.match(relayPluginSource, /backfillOwnerEmail\(\);/);
  // The relay's pairing confirmation page shows the label and platform the
  // phone declares; an empty body left it "Not stated by this app version".
  // The label is the name the phone pairs under, never the hardware model
  // (see the disclosure test below).
  assert.match(relayPluginSource, /pairing-sessions"\)\s*\.post\(RequestBody\.create\(pairingSessionBody\(DEFAULT_DEVICE_NAME\)/);
  assert.match(relayPluginSource, /DEFAULT_DEVICE_NAME = "QC Remote on Android"/);
  assert.match(relayPluginSource, /put\("platform", "android"\)/);
  // The relay creates the record the moment the code is redeemed, so a refused
  // pairing must hand it back rather than leave it on a stranger's account.
  assert.match(relayPluginSource, /withdrawCredential\(endpoint, deviceId, credential\)/);
});

test("Android pairing asks for one scan and one sign-in, with no code to repeat", () => {
  assert.doesNotMatch(relayPluginSource, /verificationCode/);
  assert.doesNotMatch(appSource, /verificationCode/);
  assert.doesNotMatch(servicesSource, /verificationCode/);
});

test("Android relay status prefers the live service and never treats saved connected state as proof", () => {
  assert.match(relayCredentialStoreSource, /private static final String RELAY_STATE = "relay_state"/);
  assert.match(relayCredentialStoreSource, /String relayState\(\)\s*\{\s*if \(!paired\(\)\) return "stopped";[\s\S]{0,180}prefs\(\)\.getString\(RELAY_STATE, "stopped"\)/);
  assert.match(relayCredentialStoreSource, /void setRelayState\(String state\)\s*\{\s*prefs\(\)\.edit\(\)\.putString\(RELAY_STATE, state\)\.apply\(\)/);
  assert.match(relayServiceSource, /state = RelayState\.STOPPED/);
  assert.match(relayServiceSource, /static String liveState\(\)/);
  // Persisted status is a hint; instance monitors cannot guard different store instances.
  assert.match(relayServiceSource, /if \(liveState != null\) return liveState;/);
  assert.match(relayServiceSource, /static String stateForStatus\(String liveState, String persistedState\)/);
  assert.match(relayServiceSource, /if \("connected"\.equals\(persistedState\)[\s\S]{0,180}return "stopped";/);
  assert.match(relayServiceSource, /new RelayCredentialStore\(this\)\.setRelayState\(value\)/);
  assert.match(relayPluginSource, /QcRelayService\.stateForStatus\(QcRelayService\.liveState\(\), store\.relayState\(\)\)/);
  assert.equal((relayPluginSource.match(/QcRelayService\.stateForStatus\(QcRelayService\.liveState\(\), store\.relayState\(\)\)/g) ?? []).length, 2,
    "both plugin load and status must use process-live service state");
});

test("Android sticky relay restart reopens a missing WebSocket despite persisted connected state", () => {
  assert.match(relayServiceSource, /if \(!shouldConnectOnStart\(state\.wireValue\(\), socket != null, socketIdentity, durable\)\) return START_STICKY;[\s\S]{0,400}generation\.incrementAndGet\(\);[\s\S]{0,400}connect\(generation\.get\(\)\)/,
    "the START_STICKY recreation path must reach connect when the process-local socket is absent");
  assert.match(relayServiceSource, /static boolean shouldConnectOnStart\(String state, boolean hasSocket\)[\s\S]{0,300}return !active \|\| !hasSocket;/,
    "only an active state backed by this service instance's WebSocket may skip reconnecting");
});

test("Android browser pairing keeps the receiver capability native and never asks the person to copy a code", () => {
  assert.match(relayPluginSource, /@PluginMethod public void beginBrowserPairing/);
  assert.match(relayPluginSource, /@PluginMethod public (?:synchronized )?void pollBrowserPairing/);
  assert.match(relayPluginSource, /private volatile BrowserPairing browserPairing/);
  assert.match(relayPluginSource, /receiverSecret/);
  assert.match(relayPluginSource, /Intent\.ACTION_VIEW/);
  assert.match(relayPluginSource, /host\/pairing-sessions/);
  assert.match(relayPluginSource, /pairWithCode\(pending\.endpoint, pairingCode/);
  assert.doesNotMatch(relayPluginSource, /result\.put\("receiverSecret"/);
});

test("Master Volume uses authoritative QC state between coalesced Android writes", () => {
  assert.match(continuousControlSource, /gateway\.setMasterVolume\(target, expected\)/);
  assert.match(continuousControlSource, /updateSnapshot\(\(current\) => \(\{\.\.\.current, masterVolume: value\}\)\)/);
  assert.doesNotMatch(continuousControlSource, /reconcile\(\{ \.\.\.(?:controller\.)?snapshotRef\.current, masterVolume: value \}\);\s*notice\(`Master Volume:/);
  assert.match(rustStateSource, /decode_master_volume/);
  assert.match(javaSource, /"master"\.equals\(kind\)[\s\S]*currentMasterVolume/);
});

test("Master Volume readback retains its own observation time", () => {
  assert.match(javaSource, /private volatile long currentMasterVolumeObservedAt = 0;/);
  assert.match(javaSource, /currentMasterVolume = volume < 0 \? -1 : \(int\) Math\.round\(volume \* 100\);[\s\S]{0,80}currentMasterVolumeObservedAt = observedAt;/);
  assert.match(javaSource, /put\("value", currentMasterVolume\)\.put\("observedAt", currentMasterVolumeObservedAt\)/);
  assert.match(javaSource, /currentMasterVolume = -1;[\s\S]{0,80}currentMasterVolumeObservedAt = 0;/);
});

test("Android I/O and Gig View mirror the physical QC screen and live assignments", () => {
  assert.doesNotMatch(javaSource, /public void swipeScreen\(PluginCall call\)/);
  // drag_screen is no longer withheld on Android: it is the same single DRAG frame as swipe_screen, which begins no held touch (source-pattern check; the Java host is not run offline).
  assert.doesNotMatch(javaSource, /"device\.dragScreen"\.equals\(method\)/);
  // A hold writes its touch-up through a writer that sends one release of its own after a later frame fails, on the same USB connection only.
  assert.match(javaSource, /if \("device\.longTapScreen"\.equals\(plan\.method\)\) \{[\s\S]*?writeHoldMessages\(plan\.messages, plan\.interMessageIntervalMs, admission\);\s*\} else \{[\s\S]*?writeMessages\(plan\.messages, plan\.interMessageIntervalMs, admission\);/);
  assert.match(javaSource, /private void writeHoldMessages\([\s\S]*connectionGeneration\.get\(\) != generation[\s\S]*writeMessage\(messages\.get\(messages\.size\(\) - 1\)\)[\s\S]*the touch-up could not be sent/);
  // mouse_frame is withheld too: a lone press leaves the pointer down and Android cannot track or release a held touch.
  assert.match(javaSource, /"device\.mouseFrame"\.equals\(method\)\s*\? failedRelay\("METHOD_NOT_ALLOWED", "mouse_frame is not supported by the Android host[^"]*held[^"]*"\)\s*: relayScreenGesture\(method, params\)/);
  assert.doesNotMatch(javaSource, /public void tapScreenDirect\(PluginCall call\)/);
  assert.doesNotMatch(servicesSource, /swipeScreen\(options:/);
  assert.match(rustScreenSource, /pub fn screen_drag[\s\S]{0,180}ScreenMouseType::Drag/);
  assert.match(rustScreenSource, /Self::Drag => pa::remote_control_mouse::Type::Drag/);
  assert.match(javaSource, /writeMessages\(plan\.messages, plan\.interMessageIntervalMs, admission\)/);
  assert.match(javaSource, /interMessageIntervalMs > 0[\s\S]*Thread\.sleep\(interMessageIntervalMs\)/);
  assert.match(rustRuntimeRequestSource, /operation_inter_message_interval_ms[\s\S]*REMOTE_GESTURE_INTERVAL_MS/);
  assert.match(usbProfileSource, /REMOTE_GESTURE_INTERVAL_MS = 20L/);
  assert.doesNotMatch(appSource, /QC_DEVICE_SCREEN_GESTURES/, "app-local I/O navigation is semantic and does not depend on physical screen coordinates");
  assert.match(readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-device-views.ts", import.meta.url), "utf8"), /showDeviceView\(view, true\)/);
  assert.match(corOsGigViewSource, /snapshot\.footswitchModes\?\.\[index < 4 \? 0 : 1\]/);
  assert.match(corOsGigViewSource, /listedPresets\.find\(\(candidate\) => candidate\.position === position\)/);
});

test("Android swipe uses the shared guarded single-frame plan without mutation replay", () => {
  // One DRAG frame, no begin or end frame (observed to scroll on CorOS 4.1.0, 2026-10-06); the Java writer takes the plan's messages and interval unchanged.
  assert.match(rustScreenSource, /pub fn screen_drag\(x: f32, y: f32, to_x: f32, to_y: f32\) -> \[OutboundMessage; 1\]/);
  assert.match(rustScreenSource, /pub fn screen_drag\([\s\S]{0,240}ScreenMouseType::Drag/);
  assert.match(javaSource, /relayScreenGesture\(method, params\)[\s\S]*relayPlannedGatewayWriteWithReadback\(method, params\)/);
  assert.match(javaSource, /writeMessages\(plan\.messages, plan\.interMessageIntervalMs, admission\)/);
  assert.match(javaSource, /for \(int index = 0; index < messages\.size\(\); index\+\+\)[\s\S]*writeMessage\(messages\.get\(index\), includeReportId,[\s\S]{0,120}stateDecoder\.prepareDispatch\(admission, monotonicMillis\(\)\)\)[\s\S]*Thread\.sleep\(interMessageIntervalMs\)/);
  assert.doesNotMatch(rustRuntimeRequestSource, /"device\.tapScreen" \| "device\.swipeScreen" => Some\("device\.captureScreen"\)/);
  assert.match(rustRuntimeRequestSource, /value\.is_finite\(\) && value\.fract\(\) == 0\.0/);
  assert.doesNotMatch(javaSource, /relayScreenGesture[\s\S]{0,500}relayReconnect/);
});

test("Android reads retain one session and deadline across retry and follow-up", () => {
  assert.doesNotMatch(javaSource, /restoreUsbSessionAfterHighVolumeRead|USB session refreshed for high-volume read|USB session recovered after read interruption/);
  assert.match(javaSource, /stateDecoder\.beginGatewayReadScope\(method, readParams, monotonicMillis\(\)\)/);
  assert.match(javaSource, /relayGatewayReadOnCurrentSession\(method, readParams, scope, relayDeadline\)/);
  assert.match(javaSource, /plan\.followupMethod, new org\.json\.JSONObject\(\), scope, relayDeadline/);
  assert.match(javaSource, /stateDecoder\.gatewayReadScopeRemaining\(pending\.scope, monotonicMillis\(\)\)/);
  assert.match(nativeDecoderSource, /nativeGatewayReadScopeRemaining/);
  assert.match(rustAndroidSource, /owner\.gateway_read_scope_remaining\(&scope, now\(\)\)\?/);
});

test("Android single-flights the uncorrelated live-screen response", () => {
  assert.match(javaSource, /captureScreenGate\.tryAcquire\(\)/);
  assert.match(javaSource, /READ_IN_PROGRESS[\s\S]{0,100}screen capture is already in progress/);
  assert.match(javaSource, /read\.whenComplete\(\(ignored, error\) -> captureScreenGate\.release\(\)\)/);
  assert.match(
    javaSource,
    /if \(pendingOperations\.remove\(entry\)\) entry\.result\.complete\(value\)/,
  );
});

test("Android preserves diagnostic counters by not reconnecting and retrying the read", () => {
  assert.match(
    javaSource,
    /if \("device\.diagnostics"\.equals\(method\)[\s\S]{0,180}\|\| !\(cause instanceof RelayException\)/,
  );
});

test("Android exposes the latest rejected gateway frame correlation in diagnostics", () => {
  assert.match(javaSource, /lastGatewayReadMismatch/);
  assert.match(javaSource, /lastGatewayReadMismatch = "type " \+ messageType/);
});

test("Android exposes physical USB health and worst-case write latency through remote status", () => {
  assert.match(javaSource, /systemStatus\(isReady\(\),[\s\S]{0,160}, usbDiagnostics\(\)\)\)/);
  assert.match(javaSource, /\.put\("usbDiagnostics", usbDiagnostics\)/);
  // Android's org.json serialises a raw String[] through Object.toString();
  // every status consumer reads capabilities as a JSON array of strings.
  assert.match(javaSource, /\.put\("capabilities", capabilitiesJson\(\)\)/);
  assert.doesNotMatch(javaSource, /\.put\("capabilities", GeneratedGatewayMethods\.CAPABILITIES\)/);
  assert.match(javaSource, /maxHidWriteDurationMs = Math\.max\(maxHidWriteDurationMs, lastHidWriteDurationMs\)/);
  assert.match(javaSource, /result\.put\("maxHidWriteDurationMs", maxHidWriteDurationMs\)/);
  assert.match(javaSource, /result\.put\("maxMidiQueueDelayMs", maxMidiQueueDelayMs\)/);
  assert.match(javaSource, /result\.put\("readerRequestActive", inputRequests != null && inputRequests\.length > 0\)/);
});

test("Android retries a timed-out read only inside its original scope", () => {
  const retry = javaSource.slice(javaSource.indexOf("relayGatewayReadWithRetry("), javaSource.indexOf("private static Throwable unwrapCompletion"));
  assert.match(retry, /"READBACK_TIMEOUT"\.equals\(\(\(RelayException\) cause\)\.code\)/);
  assert.match(retry, /gatewayReadScopeRemaining\(scope, monotonicMillis\(\)\);[\s\S]{0,150}relayGatewayReadOnCurrentSession\(method, readParams, scope, relayDeadline\)/);
  assert.doesNotMatch(retry, /relayReconnect|gatewayReadRecoveries\+\+/);
  assert.doesNotMatch(javaSource, /gatewayReadRecoveries|gatewayWriteRecoveries/);
  assert.doesNotMatch(rustRuntimeRequestSource, /"device\.tapScreen" \| "device\.swipeScreen" => Some\("device\.captureScreen"\)/);
  assert.match(javaSource, /relayScreenGesture[\s\S]{0,300}relayPlannedGatewayWriteWithReadback\(method, params\)/);
});

test("device-name writes send one frame with no readback and no replay", () => {
  // The shared policy plans no readback for setDeviceName, and the Android relay
  // sends the single Version UPDATE on the plain single-frame lane: no identity
  // readback ladder, so the result stays accepted_unverified.
  assert.doesNotMatch(rustRuntimeRequestSource, /"device\.setDeviceName" => Some\("device\.identity"\)/);
  assert.doesNotMatch(rustRuntimeRequestSource, /"device\.setDeviceName" => \{[\s\S]{0,80}supplied_fields_match/);
  assert.match(javaSource, /relaySetDeviceName\(org\.json\.JSONObject params\)[\s\S]{0,160}relayPlannedGatewayWrite\("device\.setDeviceName", params\)/);
  assert.doesNotMatch(javaSource, /result\.getJSONObject\("readback"\)/);
});

test("Android write verification stays in its original owner and never reconnects on timeout", () => {
  assert.doesNotMatch(javaSource, /recoverGatewayWriteVerification|verifyGatewayWriteAfterReconnect|authoritative_reconnect_readback/);
  assert.doesNotMatch(rustAndroidSource, /gateway_verifications: Mutex|nativeGatewayTransactionState/);
  assert.match(rustAndroidSource, /\.begin_gateway_verification\(/);
  assert.match(rustAndroidSource, /\.advance_gateway_verification\(/);
  assert.match(nativeDecoderSource, /plan\.verificationJson, plan\.sessionFence, plan\.method/);
  // No verification refresh READ any more: the shared sequencer never asks the host to send a frame.
  assert.doesNotMatch(javaSource, /dispatchGatewayRefresh|case "refresh":/);
  assert.doesNotMatch(rustAndroidSource, /GatewayVerificationAction::Refresh/);
  // An expired window is accepted_unverified, not an error and not a quarantine.
  assert.match(completionSource, /case "timedOut":[\s\S]{0,1200}"accepted_unverified"/);
  assert.match(javaSource, /case "timedOut":[\s\S]{0,1200}QcGatewayCompletion\.assemble\(/);
  assert.doesNotMatch(completionSource, /case "timedOut":[\s\S]{0,1400}(?:READBACK_TIMEOUT|quarantineWrites)/);
});

test("setlist duplication uses the shared copied-inventory verifier without replay", () => {
  assert.match(rustCommandsSource, /pub fn duplicate_setlist[\s\S]{0,900}IsFactory::IsFactory\(false\)/);
  assert.match(presetCatalogSource, /duplicate_setlist_contents_mismatch\(&copy.expected_presets, &projection\)/);
  assert.match(presetCatalogSource, /let \[created\] = candidates.as_slice\(\) else/);
  assert.match(deviceBrokerRpcSource, /execute_preset_workflow\("device.duplicateSetlist", params\)/);
  assert.match(androidPresetSource, /advance_preset_preparation\(preparation, now_ms\)/);
  assert.match(androidPresetSource, /advance_preset_workflow\(workflow, now_ms\)/);
});

test("preset rename and copy use owner-controlled cleanup after catalog verification", () => {
  assert.match(rustRuntimeRequestSource, /pub cleanup_stages: Vec<PresetMutationStage>/);
  assert.match(presetCoreSource, /PresetWorkflowCompletion::Catalog/);
  assert.match(presetOwnerSource, /skip_restoration/);
  assert.match(presetOwnerSource, /operation_in_flight/);
  assert.match(presetOwnerSource, /entry.source_checkpoint.matches\(&self.state\)/);
  assert.doesNotMatch(javaSource, /executeRelayWorkflowStages|finalizeRelayWorkflow/);
  assert.doesNotMatch(deviceBrokerRpcSource, /fn execute_preset_mutation/);
});

test("native deletion supplies proof without installing a presentation overlay", () => {
  assert.match(presetCatalogSource, /if !self\.explicit_product_deletion/);
  assert.match(presetCatalogSource, /listing\.files\.len\(\) != 256/);
  assert.match(presetCatalogSource, /file\.product_key\.as_deref\(\) == Some\(&deletion\.product_key\)/);
  const complete = presetOwnerSource.indexOf("CatalogVerificationAction::Complete =>");
  const resolution = presetOwnerSource.indexOf("entry.catalog_result = catalog.runtime.resolution()", complete);
  assert.ok(complete >= 0 && resolution > complete);
  assert.doesNotMatch(presetOwnerSource, /finalize_verified_preset_deletion|record_saved/);
  assert.doesNotMatch(nativeDecoderSource, /nativeFinalizeVerifiedPresetDeletion|nativeRecordSavedPreset/);
  assert.match(androidPresetSource, /preset_workflow_result\(workflow\)/);
});

test("modern Android keeps a buffered interrupt-read ring queued across idle periods", () => {
  assert.match(javaSource, /Build\.VERSION\.SDK_INT >= Build\.VERSION_CODES\.O/);
  assert.match(javaSource, /@RequiresApi\(Build\.VERSION_CODES\.O\)[\s\S]*readInputReportsAsync/);
  assert.match(javaSource, /HID_INPUT_REQUEST_DEPTH = 32/);
  assert.match(javaSource, /UsbRequest\[\] requests = new UsbRequest\[HID_INPUT_REQUEST_DEPTH\]/);
  assert.match(javaSource, /requests\[index\]\.queue\(buffer\)[\s\S]*activeConnection\.requestWait\(\)/);
  assert.match(javaSource, /completed\.queue\(buffer\)/);
  assert.doesNotMatch(javaSource, /requestWait\(\d+/);
  assert.match(javaSource, /inputRequest\.cancel\(\)[\s\S]*inputRequest\.close\(\)[\s\S]*releaseInterface/);
  assert.match(javaSource, /activeConnection\.bulkTransfer\(activeEndpoint[\s\S]*readerIsActive/);
});

test("Android rejects teardown-shortened HID reads before JNI and JNI cannot unwind", () => {
  assert.match(javaSource, /if \(count < 2 \|\| count > buffer\.length\)[\s\S]{0,220}Discarded truncated QC HID report/);
  const reportDecoder = rustAndroidSource.match(/pub extern "system" fn Java_org_freevia_qcremote_QcNativeStateDecoder_nativePushReport\([\s\S]*?(?=\n#\[no_mangle\])/);
  assert.ok(reportDecoder, "native report decoder entrypoint must exist");
  assert.match(reportDecoder[0], /std::panic::catch_unwind\(std::panic::AssertUnwindSafe/);
  assert.match(reportDecoder[0], /unwrap_or_else\(\|_\| Err\("native QC report decoder panicked"/);
  assert.match(rustTransportRuntimeSource, /if raw\.len\(\) < 2 \|\| raw\.len\(\) > framing::REPORT_BODY_SIZE/);
  assert.match(rustTransportRuntimeSource, /fn every_native_read_length_is_total/);
});

test("Android explains the Remote notification before requesting permission", () => {
  assert.match(relayPluginSource, /Allow Remote notifications\?/);
  assert.match(relayPluginSource, /Android requires one low-priority, ongoing notification/);
  assert.match(relayPluginSource, /USB control does not use notifications/);
  assert.match(relayPluginSource, /setNegativeButton\("Not now"/);
  assert.match(relayPluginSource, /setPositiveButton\("Continue"/);
});

test("Android relay does not disclose the hardware model as its default device name", () => {
  assert.match(relayPluginSource, /call\.getString\("deviceName", "QC Remote on Android"\)/);
  assert.doesNotMatch(relayPluginSource, /Build\.MODEL/);
});

test("Android relay preserves native error codes across completion wrappers", () => {
  const gateway = javaSource.slice(javaSource.indexOf("public void gatewayInvoke(PluginCall call)"), javaSource.indexOf("private void openAndHandshake"));
  assert.match(gateway, /error = unwrapCompletion\(error\);\s*if \(error == null\)/);
  assert.match(gateway, /error instanceof RelayException[\s\S]*"retryable", relayError\.retryable\(\)[\s\S]*call\.reject\(error\.getMessage\(\), relayError\.code, relayError, data\)/);
  assert.match(relayServiceSource, /Throwable cause = unwrapCompletion\(error\)/);
  assert.match(relayServiceSource, /cause instanceof RelayException\s*\? RelayProtocol\.error\(id, \(RelayException\) cause\)/);
  assert.match(relayProtocolSource, /error\(id, failure\.code, failure\.getMessage\(\), failure\.retryable\(\)\)/);
  assert.match(relayExceptionSource, /boolean retryable\(\)[\s\S]*"DEVICE_TRANSPORT"\.equals\(code\)/);
  assert.match(relayServiceSource, /while \(cause instanceof java\.util\.concurrent\.CompletionException/);
  assert.doesNotMatch(relayServiceSource, /Log\.\w+\([^;]*(?:Authorization:\s*Bearer|Bearer\s+[A-Za-z0-9._~-]{12,}|(?:peerToken|accessToken|credential)\s*=\s*["'][^"']{8,})/i);
});

test("Android catalog selectors distinguish JSON null from literal folder names", () => {
  assert.match(javaSource, /"device\.irs"\.equals\(method\) \? \(params\.isNull\("folder"\) \? "" : params\.getString\("folder"\)\)/);
  const firstText = javaSource.slice(javaSource.indexOf("private static String firstText"), javaSource.indexOf("private static int firstInt"));
  assert.match(firstText, /if \(value\.isNull\(key\)\) continue;[\s\S]*value\.optString\(key, ""\)/);
});

test("Android serves block details from the pushed state mirror without a forced READ", () => {
  assert.match(javaSource, /case "BLOCK_DETAILS": return relayBlockDetails\(method, params\)/);
  const details = javaSource.slice(javaSource.indexOf("private CompletableFuture<org.json.JSONObject> relayBlockDetails("), javaSource.indexOf("private CompletableFuture<org.json.JSONObject> relayGatewayReadWithRetry("));
  assert.match(details, /stateDecoder\.assertExpectedPreset\(params\)[\s\S]{0,1500}stateDecoder\.blockDetails\(row, column, params\)[\s\S]{0,200}stateDecoder\.laneControlDetails\(row, control, params\)/);
  assert.doesNotMatch(details, /relayGatewayRead\(|writeMessage|operationTicket/);
  assert.doesNotMatch(javaSource, /PendingBlockDetailsRead|resolvePendingBlockDetailsReads/);
  assert.match(rustAndroidSource, /nativeBlockDetails[\s\S]{0,1400}assert_expected_preset\(Some\(current\), &params\)\?[\s\S]{0,500}if !current\.grid_layout_complete[\s\S]{0,500}\.block_details/);
  // Windows serves the same state: no refresh function exists any more.
  assert.doesNotMatch(deviceBrokerRpcSource, /refresh_current_preset_state/);
  assert.match(deviceBrokerRpcSource, /fn gateway_block_details[\s\S]{0,1400}state_block_details\(controller, params\)/);
  assert.match(deviceBrokerRpcSource, /fn gateway_lane_control_details[\s\S]{0,800}guarded_lane_control_details\(row, control, params\)/);
});

test("both hosts serve the published settings families from the shared fold", () => {
  // CorOS publishes general and I/O settings at handshake and then answers
  // writes with a sparse echo; it never answers an on-demand READ of type 9
  // or type 3. Android used to issue those reads and wait out the full
  // timeout for data the boot publication already held.
  const dispatch = JSON.parse(
    readFileSync(new URL("../../../contracts/public/gateway-methods.v1.json", import.meta.url), "utf8")
  ).androidDispatch;
  assert.deepEqual(dispatch.GENERAL_SETTINGS, ["device.generalSettings"]);
  assert.deepEqual(dispatch.IO_SETTINGS, ["device.ioSettings"]);
  assert.ok(!dispatch.CORRELATED_READ.includes("device.generalSettings"));
  assert.ok(!dispatch.CORRELATED_READ.includes("device.ioSettings"));
  // The retention and the merge are shared Rust, so neither host can drift.
  assert.match(rustStateRuntimeSource, /fn retain_settings_publication/);
  assert.match(rustStateRuntimeSource, /merge_general_settings_publication/);
  assert.match(rustStateRuntimeSource, /merge_io_settings_publication/);
  assert.match(rustStateRuntimeSource, /pub fn published_settings/);
  assert.match(javaSource, /case "GENERAL_SETTINGS"[\s\S]{0,220}publishedSettings\(/);
  assert.match(javaSource, /case "IO_SETTINGS"[\s\S]{0,220}publishedSettings\(/);
  // The write-readback path must use the same fold. Asking the wire for a
  // family the QC never answers waits out the whole budget after a write that
  // in fact succeeded.
  assert.match(javaSource, /private static int publishedSettingsType\(String readMethod\)/);
  assert.match(javaSource, /publishedSettingsType\(readMethod\)[\s\S]{0,1200}operationObservationIsFresh\(echoFence\.operation, echo\.observationTicket\)[\s\S]{0,300}gatewayWriteEchoMatches\(method, params, published, echo\.payload\)[\s\S]{0,300}stateDecoder\.publishedSettings\(published\)/);
});

test("no preflight sends a READ: both hosts compare with decoded state and fail closed", () => {
  // Cortex Control reads nothing before a write. Every guard reads state the session already
  // folds; one whose state has not been published fails closed and sends nothing.
  assert.match(javaSource, /QcGatewayPreflight\.authoritativeObservation\(\s*plan\.preflightObservationSource,\s*\(\) -> stateDecoder\.preflightObservation\(method\)\)/);
  assert.match(javaSource, /bindGatewayWritePreflight\(plan, params, observation\)/);
  assert.doesNotMatch(javaSource, /beginGatewayReadScope\(\s*plan\.preflightMethod|relayGatewayReadOnCurrentSession\(plan\.preflightMethod/);
  assert.match(deviceBrokerRpcSource, /gateway_preflight_observation\(method\)/);
  assert.match(deviceBrokerRpcSource, /gateway_preflight_unavailable_message\(method\)/);
  assert.doesNotMatch(deviceBrokerRpcSource, /fn preflight_response|CorrelatedReply/);
  assert.match(rustRuntimeRequestSource, /pub enum GatewayPreflightObservationSource \{\s*\/\/\/[^\n]*\n\s*DecodedSnapshot,\s*\}/);
});

test("Android catalog refresh refusal uses the generated shared policy text", () => {
  // An explicit refresh is not implemented (the unit does answer a directory read, ESTABLISHED
  // 2026-10-07). Android used to buy a whole new USB session (~12 s handshake) behind the
  // caller's back instead of saying so.
  assert.match(javaSource, /GeneratedHostPolicy\.PRESET_CATALOG_REFRESH_UNAVAILABLE/);
  assert.doesNotMatch(javaSource, /private static final String PRESET_CATALOG_REFRESH_UNAVAILABLE =/);
  assert.doesNotMatch(javaSource, /relayReconnect\("USB session refreshed for preset catalog"/);
  assert.match(deviceBrokerRpcSource, /PRESET_CATALOG_REFRESH_UNAVAILABLE/);
});

test("Android waits for the published settings echo without sending anything or replaying the mutation", () => {
  assert.match(javaSource, /stateDecoder\.gatewayPlan\(method, JSObject\.fromJSONObject\(params\)\)/);
  assert.match(javaSource, /bindGatewayWritePreflight\(plan, params, observation\)/);
  assert.match(javaSource, /PlannedGatewayWrite admittedPlan = bound\.plan/);
  assert.match(javaSource, /executeRelayPlan\(admittedPlan, -1, echoFence\)\.thenCompose\(writeResult -> admittedPlan\.noOp \|\| admittedPlan\.readbackMethod == null/);
  assert.match(javaSource, /stateDecoder\.gatewayReadbackRetryDelay\(method, attempt\)/);
  assert.match(nativeDecoderSource, /nativeGatewayReadbackRetryDelay/);
  assert.match(rustRuntimeRequestSource, /CORRELATED_WRITE_READBACK_RETRY_INTERVALS_MS\.to_vec\(\)/);
  assert.match(javaSource, /relayGatewayWriteReadback[\s\S]{0,2700}gatewayReadbackMatches/);
  assert.doesNotMatch(
    javaSource,
    /relayGatewayWriteReadback[\s\S]{0,2700}(?:writeMessages|relayPlannedGatewayWrite|relayGatewayReadOnCurrentSession)\(/,
    "readback recovery must never replay a potentially applied settings mutation or send a read",
  );
});

test("Android provides a configuration-independent splash fallback", () => {
  assert.match(splashFallbackSource, /<shape[\s\S]*<solid android:color="@color\/brandBackground"/);
});

test("Android bridge logs never expose relay credentials or pairing codes", () => {
  assert.equal(JSON.parse(capacitorConfigSource).loggingBehavior, "none");
});

test("Android reconnects on attachment and recovers an unexpected reader exit", () => {
  assert.match(javaSource, /ACTION_USB_DEVICE_ATTACHED/);
  assert.doesNotMatch(javaSource, /scheduleAutomaticReconnect\("Quad Cortex USB reattached"\)/);
  assert.match(
    javaSource,
    /systemGrantedAttachment[\s\S]{0,900}pendingConnect[\s\S]{0,220}openAndHandshake\(candidate, waiting\)/,
    "the system attachment grant must complete a connect request that was waiting behind its dialog",
  );
  assert.match(appSource, /if \(state === "available"\)[\s\S]{0,120}attemptUsbConnection\(\)/);
  assert.match(javaSource, /boolean recoverReader = readerIsActive\(activeConnection, generation\)/);
  assert.match(javaSource, /sessionTerminalReadFailed\(\)[\s\S]{0,300}terminalTransportFailed\(/);
  assert.match(javaSource, /terminalTransportFailed[\s\S]{0,300}closeConnection\(null, false\)[\s\S]{0,200}scheduleAutomaticReconnect\(detail\)/);
  assert.match(javaSource, /catch \(Exception error\)[\s\S]{0,700}recoverUnexpectedReaderExit\(activeConnection, generation\)/);
  assert.match(javaSource, /sessionScheduleReconnect\(monotonicMillis\(\)\)/);
  assert.match(javaSource, /sessionReconnectDue\(now\)/);
  assert.match(javaSource, /sessionReconnectAttempted\(now\)/);
  assert.match(androidPresetSource, /lifecycle.reserve_request_id\(\)/);
  assert.doesNotMatch(nativeDecoderSource, /nativeNextRequestId/);
  assert.doesNotMatch(javaSource, /AtomicLong requestIds/);
  assert.doesNotMatch(javaSource, /initializationObserved\(/);
  assert.match(rustAndroidSource, /runtime\.take_synchronization_change\(\)[\s\S]{0,300}\.synchronization_completed\(now_ms, synchronized\)/);
  assert.match(rustSessionSource, /self\.invalidate_work\(\)[\s\S]{0,650}self\.set_synchronization\(false\)/);
  assert.doesNotMatch(javaSource, /backupActive \|\| !pendingOperations\.isEmpty\(\)/);
  assert.doesNotMatch(javaSource, /scheduleAutomaticReconnect[\s\S]{0,800},\s*250,\s*TimeUnit\.MILLISECONDS/);
});

test("Android bounds ambiguous SET_REPORT failures by independent session liveness", () => {
  assert.match(javaSource, /QcHidWritePolicy\.classifyAndPropagate\([\s\S]{0,500}this::terminalHidWriteFailed/);
  assert.match(javaSource, /readerTerminatedGeneration == activeGeneration/);
  assert.match(javaSource, /isDeviceStillAttached\(activeDevice\)/);
  assert.match(hidWritePolicySource, /if \(result < 0\) return Outcome\.EXPECTED_STATUS_STALL/);
  assert.match(hidWritePolicySource, /if \(!deviceAttached\) return Outcome\.TERMINAL/);
  assert.match(hidWritePolicySource, /if \(readerTerminated\) return Outcome\.TERMINAL/);
  assert.match(hidWritePolicyTestSource, /twoFragmentStatusStallsRemainDelivered/);
  assert.match(hidWritePolicyTestSource, /detachIsTerminalAndPropagatesRecoverySignal/);
  assert.match(hidWritePolicyTestSource, /receiveLaneCessationIsTerminalAndPropagatesRecoverySignal/);
});

test("Android persists a bounded payload-free USB flight recorder", () => {
  const recorder = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcUsbFlightRecorder.java", import.meta.url), "utf8");
  assert.match(javaSource, /new QcUsbFlightRecorder\(getContext\(\)\)/);
  assert.match(javaSource, /flight\.event\("handshake-started"\)/);
  assert.match(javaSource, /flight\.event\("handshake-reply"\)/);
  assert.match(javaSource, /flight\.outbound\(message\.messageType, framedReports\.size\(\)\)/);
  assert.match(javaSource, /flight\.inbound\(decoded\.messageType\)/);
  assert.match(javaSource, /result\.put\("flightRecorder", flight\.snapshot\(\)\)/);
  assert.match(recorder, /MAX_ENTRIES = 256/);
  assert.match(recorder, /qc-usb-flight-recorder\.json/);
  assert.match(recorder, /if \(critical\)[\s\S]{0,300}else if \(now - lastPersistedAt >= CHECKPOINT_INTERVAL_MS\)/);
  assert.match(recorder, /persistenceIo\.execute\(this::drainPersistQueue\)/,
    "routine traffic persistence must stay off the USB command and reader lanes");
  assert.match(recorder, /private void drainPersistQueue\(\)[\s\S]*synchronized \(fileLock\) \{ persist\(snapshot\); \}/);
  assert.match(recorder, /closed = true;[\s\S]{0,240}awaitTermination[\s\S]{0,300}persist\(snapshot\)/,
    "shutdown must drain older checkpoints before the final current snapshot");
  assert.match(recorder, /MAX_FILE_BYTES = 256 \* 1024L/);
  assert.match(recorder, /entries\.addLast\(sanitize\(entry\)\)/);
  assert.match(recorder, /isRoutineLiveness[\s\S]*MESSAGE_TYPE_KEEP_ALIVE/);
  assert.doesNotMatch(recorder, /isRoutineLiveness[\s\S]{0,250}MESSAGE_TYPE_VERSION/);
  assert.match(javaSource, /flight\.activity\(GeneratedGatewayMethods\.contains\(method\)[\s\S]{0,100}"gateway-dispatch:" \+ method/);
  assert.doesNotMatch(javaSource, /gateway-dispatch:" \+ (?:params|expected)/);
  assert.doesNotMatch(recorder, /entry\.put\("(?:payload|serial|deviceName|credential|token)"/i);

  const systemStatus = javaSource.slice(
    javaSource.indexOf('if ("SYSTEM".equals(dispatch))'),
    javaSource.indexOf('if ("RECONNECT".equals(dispatch))'),
  );
  assert.match(systemStatus, /usbDiagnostics\(\)/);
  assert.doesNotMatch(systemStatus, /flightRecorder|flight\.snapshot/,
    "remote system.status must not disclose the local flight record");
});

test("Android's USB maintenance uses the same dedicated shared KeepAlive as Windows", () => {
  const start = javaSource.indexOf("keepalive.scheduleWithFixedDelay");
  const end = javaSource.indexOf("static boolean relaySessionAvailable", start);
  const maintenance = javaSource.slice(start, end);
  assert.match(maintenance, /sessionShouldKeepalive/);
  assert.doesNotMatch(maintenance, /pendingOperations\.isEmpty\(\)/);
  assert.match(maintenance, /stateDecoder\.keepaliveCommand\(\)/);
  assert.match(maintenance, /stateDecoder\.sessionKeepaliveSent\(monotonicMillis\(\)\)/);
  assert.doesNotMatch(javaSource, /stateDecoder\.session\w+\(System\.currentTimeMillis\(\)/);
  assert.doesNotMatch(maintenance, /readCommand\(QcUsbProfile\.MESSAGE_TYPE_VERSION\)/);
  assert.match(javaSource, /MAINTENANCE_POLL_MS = 1000/);
  assert.match(javaSource, /MAINTENANCE_POLL_MS, MAINTENANCE_POLL_MS, TimeUnit\.MILLISECONDS/);
  assert.match(javaSource, /return connection != null && stateDecoder\.sessionConnected\(\)/);
  assert.doesNotMatch(javaSource, /handshakeComplete|stateSynchronized|initializationComplete/);
  assert.match(javaSource, /MESSAGE_TYPE_RESET_COMMS_BUFFERS[\s\S]{0,120}decoded\.lifecycle\.sessionValidated/);
  assert.doesNotMatch(javaSource, /"versionValidating"\.equals\(startup\.phase\)/);
  assert.match(rustInitializationSource, /SessionValidating[\s\S]*commands::read_version\(\)/);
  assert.doesNotMatch(javaSource, /keepalive\.schedule\([\s\S]{0,500}readCommand\(QcUsbProfile\.MESSAGE_TYPE_VERSION\)/);
});

test("Android state-event reads expose the true tail beyond a paged frame window", () => {
  assert.match(javaSource, /result\.put\("latestSequence", latestSequence\)/);
  assert.match(javaSource, /latestSequence = nextStateSequence - 1/);
});

test("Android history writes are one frame with no refresh read and no replay", () => {
  assert.match(javaSource, /"device\.undo"\.equals\(method\) \|\| "device\.redo"\.equals\(method\)[\s\S]{0,120}\? relayPlannedGatewayWrite\(method, params\)/);
  assert.doesNotMatch(javaSource, /relayHistoryWrite/);
  assert.doesNotMatch(javaSource, /postWriteRefresh/);
  assert.doesNotMatch(nativeDecoderSource, /postWriteRefresh/);
  assert.doesNotMatch(javaSource, /currentPresetCommand/);
});

test("Android sends one backup export request and never replays it", () => {
  // Windows collects the document on its device loop in worker.rs; Android's
  // equivalent lives in QcUsbPlugin. Both send one uncorrelated export request
  // and never splice streams or replay that request.
  assert.match(rustAndroidSource, /"started": progress\.started/);
  assert.match(rustAndroidSource, /"ignoredPrefixChunks": progress\.ignored_prefix_chunks/);
  assert.match(javaSource, /scheduleBackupWatchdog\(pending, MAINTENANCE_POLL_MS, relayDeadline\)/);
  assert.match(usbProfileSource, /BACKUP_FIRST_CHUNK_TIMEOUT_MS = 60000L/);
  assert.match(usbProfileSource, /BACKUP_STREAM_STALL_TIMEOUT_MS = 15000L/);
  assert.match(javaSource, /stateDecoder\.backupAdvance\(operation\.admission, monotonicMillis\(\)\)/);
  assert.doesNotMatch(javaSource, /"rerequest"\.equals\(action\)/);
  assert.match(javaSource, /"failed"\.equals\(action\)[\s\S]*stateDecoder\.backupCancelled\(pending\.operation\.admission\)/);
  assert.match(javaSource, /Sending single native backup request/);
  assert.match(rustBackupRuntimeSource, /export request was not replayed/);
  assert.doesNotMatch(rustBackupRuntimeSource, /BackupAction::Rerequest/);
  assert.match(javaSource, /stateDecoder\.operationTicket\(startedAt, QcUsbProfile\.BACKUP_TOTAL_TIMEOUT_MS\)/);
  assert.match(javaSource, /stateDecoder\.backupStarted\(admission, startedAt\)/);
  assert.match(javaSource, /issueBackupRequest\(pending, admission, relayDeadline\)/);
});

test("Android preset writes are confirmed passively without replaying the mutation", () => {
  const verificationPolicy = rustRuntimeRequestSource.slice(
    rustRuntimeRequestSource.indexOf("pub fn gateway_write_verification_policy"),
    rustRuntimeRequestSource.indexOf("pub fn gateway_verification_policy"),
  );
  assert.match(verificationPolicy, /"device\.recallPreset"[\s\S]*"device\.navigateBank"[\s\S]*"device\.reloadPreset"/);
  assert.match(javaSource, /stateDecoder\.gatewayVerificationStarted\(/);
  assert.doesNotMatch(javaSource, /dispatchGatewayRefresh|case "refresh":/);
  assert.doesNotMatch(javaSource, /registered != null && plan\.retryable/);
  assert.doesNotMatch(javaSource, /writeMessage\(message, !includeReportId\)/);
  assert.doesNotMatch(javaSource, /isIdempotentGatewayWrite/);
});

test("live QC parameter frames update the open shared editor", () => {
  assert.match(generatedPayloadSource, /\| "parameter"/);
  // decode_grid's three parameter-override sweeps (model, lane control,
  // splitter/mixer) share record_parameter_overrides(), which takes the
  // StateUpdate kind as a parameter rather than each sweep inlining its own
  // literal StateUpdate::new("parameter") call.
  assert.match(rustStateSource, /fn record_parameter_overrides[\s\S]{0,900}StateUpdate::new\(kind\)/);
  assert.match(rustStateSource, /"parameter",\s*&mut states,/);
  assert.match(rustStateSource, /parameter_overrides[\s\S]{0,100}\.insert/);
  assert.match(appSource, /useQcLiveState/);
  assert.match(liveStateSource, /state\.kind === "parameter"[\s\S]*editor\.updateParameters/);
});

test("Android queues relay work behind an in-flight USB reconnect", () => {
  assert.match(javaSource, /private volatile CompletableFuture<org\.json\.JSONObject> reconnectInFlight;/);
  assert.match(javaSource, /if \(reconnectInFlight != null\) return reconnectInFlight;/);
  assert.match(javaSource, /reconnect\.whenComplete\(\(ignored, reconnectError\) -> \{[\s\S]*submitRelayTask\(commandIo, relayDeadline,[\s\S]*relayInvoke\(method, deferredParams, deferredExpected\)/);
  assert.doesNotMatch(javaSource, /reconnect\.thenCompose\(ignored -> relayInvoke/);
  assert.match(javaSource, /if \(reconnectInFlight == result\) \{\s*connecting = false;\s*reconnectInFlight = null;/);
  assert.match(javaSource, /result\.whenComplete\(\(ignored, error\) -> \{[\s\S]{0,240}connecting = false;[\s\S]{0,160}reconnectInFlight = null;/);
  assert.doesNotMatch(javaSource, /openDeviceAndHandshake\(candidate, pending\);[\s\S]{0,200}finally[\s\S]{0,180}reconnectInFlight = null/);
});

test("Android sends the exact shared disconnected-state sequence before USB teardown", () => {
  assert.match(nativeDecoderSource, /List<EncodedMessage> disconnectCommands\(\)[\s\S]{0,120}commands\("disconnect"/);
  assert.match(javaSource, /if \(sendDisconnectCommands && connection != null && stateDecoder\.startupActive\(\)\)[\s\S]{0,220}writeMessages\(stateDecoder\.disconnectCommands\(\)\)/);
  const start = javaSource.search(/    private synchronized void closeConnection\(\r?\n/);
  assert.ok(start >= 0);
  const close = javaSource.slice(start, javaSource.indexOf("    @Override", start));
  const steps = ["writeMessages(stateDecoder.disconnectCommands())", "stateDecoder.sessionDisconnected(",
    "stateDecoder.reset()", "reading = false", "pendingOperations.failAllExcept("]
    .map((step) => close.indexOf(step));
  assert.ok(steps.every((offset) => offset >= 0));
  assert.ok(steps.every((offset, index) => index === 0 || offset > steps[index - 1]),
    "disconnect commands precede native invalidation, reader teardown and completion callbacks");
});

test("Android public catalog reads poll native evidence without another USB request", () => {
  const start = javaSource.indexOf("    private CompletableFuture<org.json.JSONObject> relayCatalogRead(");
  const route = javaSource.slice(start, javaSource.indexOf("    private CompletableFuture<org.json.JSONObject> relayMidi(", start));
  assert.match(route, /catalogContext\(null\)/);
  assert.match(route, /"device\.irs"\.equals\(method\) && params\.has\("folder"\)/);
  assert.match(route, /!\(params\.get\("folder"\) instanceof String\)/);
  assert.match(route, /"device\.captures"\.equals\(method\) \? ""/);
  assert.match(route, /"device\.irs"\.equals\(method\) \? \(params\.isNull\("folder"\) \? "" : params\.getString\("folder"\)\)/);
  assert.match(route, /resolvePendingCatalogRead\(pending\)/);
  assert.doesNotMatch(route, /writeMessage|operationTicket|commandIo/);
  assert.match(javaSource, /stateDecoder\.retainedCatalog\(/);
  assert.match(javaSource, /projection\.getBoolean\("ready"\)/);
  assert.match(javaSource, /Math\.max\(1L, Math\.min\(100L, remainingMs\)\)/);
  assert.doesNotMatch(javaSource, /presetFolderObservationSequence|catalogObservationCurrent|schedulePresetLibrarySettlement/);
});

test("Android refuses an explicit preset catalog refresh instead of buying a session", () => {
  assert.match(javaSource, /boolean refresh = !"device\.captures"\.equals\(method\) && !"device\.irs"\.equals\(method\)\s*&& params\.optBoolean\("refresh", false\);/);
  // An explicit refresh is not implemented, so a reconnect would buy a ~12 s handshake and
  // discard every other session-scoped observation to re-read the directory.
  assert.match(javaSource, /if \(refresh\)[\s\S]{0,1400}PRESET_CATALOG_REFRESH_UNAVAILABLE/);
  assert.match(javaSource, /stateDecoder\.catalogContext\(pending\.context\)/);
  assert.match(javaSource, /stateDecoder\.requestExpired\(pending\.startedAtMs, pending\.timeoutMs, monotonicMillis\(\)\)/);
});

test("Android write-quarantine mirrors the desktop broker's writesQuarantined/writeQuarantineReason", () => {
  // services/device-broker/src/worker.rs DeviceSession fields and
  // Gateway::quarantine_writes are the reference this state is named after.
  const admissionGuard = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayAdmissionGuard.java", import.meta.url), "utf8");
  assert.match(javaSource, /private void quarantineWrites\(String reason\) \{/);
  const quarantineMethod = javaSource.slice(
    javaSource.indexOf("private void quarantineWrites(String reason) {"),
    javaSource.indexOf("private JSObject usbDiagnostics()"));
  assert.match(quarantineMethod, /relayAdmissionGuard\.quarantineWrites\(reason\)/);
  assert.match(admissionGuard, /synchronized void quarantineWrites\(String reason\)[\s\S]*writesQuarantined = true;[\s\S]*writeQuarantineReason = reason;/);
});

test("Android exposes writesQuarantined/writeQuarantineReason from usbDiagnostics, same field names as the desktop broker", () => {
  const start = javaSource.indexOf("private JSObject usbDiagnostics() {");
  const body = javaSource.slice(start, javaSource.indexOf("private static JSObject messageCountsJson(", start));
  assert.match(body, /result\.put\("writesQuarantined", relayAdmissionGuard\.writesQuarantined\(\)\);/);
  assert.match(body, /if \(relayAdmissionGuard\.writeQuarantineReason\(\) != null\)[\s\S]*result\.put\("writeQuarantineReason", relayAdmissionGuard\.writeQuarantineReason\(\)\)/);
  assert.match(deviceBrokerRpcSource, /error_has_ambiguous_write_outcome/);
});

test("Android quarantines writes only for genuine delivery ambiguity, never on a read timeout or an expired confirmation window", () => {
  // Preset workflow: the native journal reported an error after the write step ran.
  const presetFinish = javaSource.slice(
    javaSource.indexOf('if ("finished".equals(kind)) {'),
    javaSource.indexOf('return new QcPresetWorkflowRunner.Step<>(true,'));
  assert.match(presetFinish, /quarantineWrites\(error\);/);
  assert.match(presetFinish, /throw new RelayException\("WRITE_OUTCOME_AMBIGUOUS", error\);/);

  // Write-readback verification and the general verification loop: reached only after
  // executeRelayPlan already dispatched the write. An expired window or a poll schedule that ran
  // out is NOT a failure: the frames were delivered and the QC pushed no confirming state, so the
  // caller gets accepted_unverified and nothing is quarantined.
  const readback = javaSource.slice(
    javaSource.indexOf("private CompletableFuture<org.json.JSONObject> relayGatewayWriteReadback("),
    javaSource.indexOf("private CompletableFuture<org.json.JSONObject> executeRelayPlan("));
  assert.doesNotMatch(readback, /quarantineWrites|READBACK_MISMATCH|READBACK_TIMEOUT/);
  const timedOut = completionSource.slice(completionSource.indexOf('case "timedOut":'), completionSource.indexOf("default: throw"));
  assert.doesNotMatch(timedOut, /quarantineWrites|READBACK_TIMEOUT/);
  assert.match(timedOut, /"accepted_unverified"/);

  // A plain read timeout (relayGatewayReadOnCurrentSession / catalog reads) must
  // never quarantine writes - only dispatched mutations do.
  const plainRead = javaSource.slice(
    javaSource.indexOf("private CompletableFuture<org.json.JSONObject> relayGatewayReadOnCurrentSession("),
    javaSource.indexOf("private static Throwable unwrapCompletion("));
  assert.doesNotMatch(plainRead, /quarantineWrites/);
});

test("Android clears write quarantine only on a fresh device connect, not on the next successful write", () => {
  const connectBlock = javaSource.slice(
    javaSource.indexOf("device = candidate;"),
    javaSource.indexOf("nativeConnectionToken = stateDecoder.sessionOpened("));
  assert.match(connectBlock, /relayAdmissionGuard\.resetForNewSession\(\)/);
  // The successful-verification branch of the write loop must not clear it -
  // clearing happens only on the reconnect above, mirroring the desktop
  // broker (services/device-broker/src/worker.rs) only clearing
  // writes_quarantined where a session re-synchronizes, never on an
  // individual write's own success.
  const verified = javaSource.slice(javaSource.indexOf('case "verified":'), javaSource.indexOf('case "timedOut":'));
  assert.doesNotMatch(verified, /writesQuarantined = false/);
});

test("Android reports a verified plugin add's readback the way the desktop broker does", () => {
  // The readback is decided natively, under the owner lock, against the state
  // the verification matched; Java only forwards it.
  assert.match(nativeDecoderSource, /nativeGatewayVerificationAdvance\(\s*long handle, long verificationId, long observationSequence, long nowMs,\s*String method, String verificationJson\);/);
  assert.match(javaSource, /gatewayVerificationAdvance\(\s*pending\.verificationId, pending\.observationSequence\(\), now,\s*pending\.plan\.method, pending\.plan\.verificationJson\)/);
  const verified = javaSource.slice(javaSource.indexOf('case "verified":'), javaSource.indexOf('case "timedOut":'));
  assert.match(verified, /QcGatewayCompletion\.assemble\([\s\S]*entry\.result\.complete\(completion\);/);
  assert.match(completionSource, /attachPluginBlockReadback\(completion, action\);/);
  const attach = completionSource.slice(completionSource.indexOf("static void attachPluginBlockReadback("), completionSource.indexOf("static void applyNativeOutcome("));
  assert.match(attach, /action\.optJSONObject\("pluginBlockReadback"\)/);
  assert.match(attach, /completion\.put\("pluginBlockReadback", readback\)/);
  assert.match(attach, /detail\.endsWith\("\."\) \? detail \+ " " \+ note : detail \+ "\. " \+ note/);
  assert.doesNotMatch(attach, /quarantineWrites|removeBlock|completeExceptionally|failedRelay/,
    "a locked plugin is reported, never refused or removed");
  assert.match(rustAndroidSource, /GatewayVerificationAction::Verified => \{\s*verified_action\(method, verification_json, owner\.state\.snapshot\(\)\)/);
  assert.match(rustAndroidSource, /quadcortex::device::request::verified_plugin_block_readback\(/);
  assert.match(rustRuntimeRequestSource, /pub fn verified_plugin_block_readback\(/);
  assert.doesNotMatch(javaSource, /The Quad Cortex added this plugin block bypassed/,
    "the note text has one owner, in the shared Rust request module");
});

test("Android surfaces a write-quarantine notice analogous to Windows App.tsx, without reusing disconnect/demo recovery", () => {
  assert.match(appSource, /useWriteHealth\(\{[\s\S]*recoveryInstruction: "the device is reconnected"/);
  assert.match(appSource, /androidUsbRuntimeDiagnostics\(\)/);
  assert.match(appSource, /observeWriteHealth\(Boolean\(runtimeDiagnostics\.writesQuarantined\), runtimeDiagnostics\.writeQuarantineReason\)/);
  assert.match(appSource, /onQuarantineChange: setUsbWritesQuarantined/);
  assert.match(appSource, /qcConnectionIndicatorPhase\(connection, hardwareConnected && !demoActive, null, \{ actionRequired: usbWritesQuarantined \}\)/);
  assert.doesNotMatch(appSource, /wasWritesQuarantinedRef|transitionWriteQuarantine/);
  assert.match(servicesSource, /androidUsbRuntimeDiagnostics/);
  assert.match(servicesSource, /phase, detail, connected: android\.connected, synchronized: android\.synchronized/);
  assert.match(servicesSource, /writesQuarantined: boolean; writeQuarantineReason\?: string;/);
});

test("remote deletions use initiating-client confirmation without a second phone prompt", () => {
  // The initiating AI app owns explicit persistent-write confirmation.
  for (const method of ["device.deletePreset", "device.deleteSetlist"]) {
    assert.ok(remoteActionsSource.includes(`"${method}"`), `${method} remains remotely available`);
  }
  assert.match(remoteActionsSource, /REQUIRES_CONFIRMATION = \{[\s\S]*"device\.deleteSetlist"[\s\S]*"device\.deletePreset"/);
  assert.doesNotMatch(relayCredentialStoreSource, /boolean autoApproveRemoteActions\(\)/);
  assert.doesNotMatch(relayServiceSource, /RelayApprovalDesk|approvalSummary|ACTION_APPROVAL/);
  assert.doesNotMatch(relayPluginSource, /resolveApproval|setAutoApproveRemoteActions|relayApproval/);
  assert.match(relayServiceSource, /RelayAccessPolicy\.permits/);
  assert.match(relayServiceSource, /dispatch\(webSocket, decision\.id, decision\.method, decision\.params,[\s\S]{0,100}decision\.deadlineMs\)/);
  assert.match(relayServiceSource, /gatewayApiVersion.*GeneratedGatewayMethods\.API_VERSION/);
  assert.match(appSource, /<ControlAccessSelect value=\{controlAccessMode\} ariaLabel="MCP device access"/);
});

test("Android relay lifecycle and pairing failures remain recoverable", () => {
  assert.match(javaSource, /deviceReceiver, deviceFilter, ContextCompat\.RECEIVER_NOT_EXPORTED/);
  const browserPairFailure = relayPluginSource.slice(
    relayPluginSource.indexOf('if (!response.isSuccessful()) {', relayPluginSource.indexOf('private void pairWithCode')),
    relayPluginSource.indexOf('JSONObject payload', relayPluginSource.indexOf('private void pairWithCode')),
  );
  assert.match(browserPairFailure, /browserPairingCompleting = false;[\s\S]*pairingResponseError\(response\.code\(\),heldSnapshot\.paired\(\)\)[\s\S]*call\.reject\([\s\S]*,code\)/);
  const credentialFailure = relayServiceSource.slice(
    relayServiceSource.indexOf('try { held = store.snapshot(); }'),
    relayServiceSource.indexOf('if (endpoint == null', relayServiceSource.indexOf('try { held = store.snapshot(); }')),
  );
  assert.doesNotMatch(credentialFailure, /store\.clear\(\)/);
  assert.match(credentialFailure, /RelayState\.RECONNECTING/);
  assert.match(credentialFailure, /reconnect\(attemptGeneration\)/);
  assert.match(relayServiceSource, /try \{\s*startForeground\([\s\S]*?\} catch \(RuntimeException error\)/);
});

test("Android teardown cancels handshake work and closes native state off the callback thread", () => {
  assert.match(javaSource, /handshakeGeneration = connectionGeneration\.get\(\)/);
  assert.match(javaSource, /connectionGeneration\.get\(\) != handshakeGeneration/);
  assert.match(javaSource, /transportClosing \|\| connection == null/);
  assert.match(javaSource, /transportClosing = true;/);
  assert.match(javaSource, /connection = publishHidConnection;/);
  assert.match(javaSource, /transportClosing = false;/);
  assert.match(javaSource, /private final QcRelayAdmissionGuard relayAdmissionGuard = new QcRelayAdmissionGuard\(\)/);
  assert.match(javaSource, /expectedLifecycleEpoch != relayAdmissionGuard\.epoch\(\)/);
  assert.match(javaSource, /if \(!published\)\s*\{[\s\S]*openedMidi\.close\(\)[\s\S]*opened\.close\(\)/);
  assert.match(javaSource, /relayAdmissionGuard\.markTerminal\(\)/);
  assert.match(javaSource, /Thread\.sleep\(10L\)/);
  const teardown = javaSource.slice(javaSource.indexOf('protected void handleOnDestroy()'));
  assert.match(teardown, /stopService\(new Intent\(getContext\(\), QcRelayService\.class\)\)/);
  assert.match(teardown, /lifecycleTeardown\.execute\(\(\) ->/);
  assert.match(teardown, /closeConnection\(\)/);
  assert.match(teardown, /await\(Long\.MAX_VALUE\)/);
  assert.match(teardown, /if \(readerStopped\)\s*\{\s*try \{ stateDecoder\.close\(\)/);
});

test("Android quarantine blocks device mutations but preserves reads and reconnect", () => {
  assert.match(javaSource, /relayAdmissionGuard\.writesQuarantined\(\) && isMutationDispatch\(dispatch\)/);
  const mutationPolicy = javaSource.slice(javaSource.indexOf('private static boolean isMutationDispatch'), javaSource.indexOf('private CompletableFuture<org.json.JSONObject> relayPlannedGatewayWrite'));
  for (const dispatch of ["SET_DEVICE_NAME", "TAP_SCREEN", "PREVIEW_PARAMETER", "PLANNED_WRITE", "PRESET_WRITE", "PERSISTENT_WRITE"])
    assert.ok(mutationPolicy.includes(`"${dispatch}"`), `${dispatch} must stay quarantined`);
  assert.match(javaSource, /if \("RECONNECT"\.equals\(dispatch\)\) return relayReconnect/);
});

test("Android relay invocations carry bounded deadlines and never retry an unknown outcome", () => {
  assert.match(relayServiceSource, /MAX_IN_FLIGHT_INVOKES = 8/);
  assert.match(relayServiceSource, /invocationAdmission\.tryAcquire\(\)/);
  assert.match(relayServiceSource, /relayFrameHandler\.inspect\(text/);
  assert.match(relayFrameHandlerSource, /requestDeadlineMs\(request\)/);
  assert.match(relayServiceSource, /DEVICE_OUTCOME_UNKNOWN/);
  assert.match(relayServiceSource, /Its outcome is unknown; do not retry it automatically/);
  assert.match(relayServiceSource, /expireRelayInvocation\(invocation\)/);
  assert.match(javaSource, /if \(result\.isDone\(\)\) return;[\s\S]{0,180}relayDeadline\.isExpired\(\)/);
  assert.match(javaSource, /nextPresetWorkflowAction\(preparation, relayDeadline\)/);
  assert.match(javaSource, /if \(preparationRead\) \{[\s\S]*?beginRelayRead\(relayDeadline\)[\s\S]*?\} else \{[\s\S]*?beginRelayMutation\(relayDeadline\)[\s\S]*?writeMessages\(step\.messages, interval, operation\)/);
  assert.match(javaSource, /private boolean beginRelayMutation\(RelayDeadline deadline\)/);
  assert.match(javaSource, /if \(relayAdmissionGuard\.writesQuarantined\(\)\) throw new RelayException\("WRITE_QUARANTINED"/);
  assert.match(readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayAdmissionGuard.java", import.meta.url), "utf8"), /The relay deadline expired after a device mutation may have been sent/);
  assert.match(javaSource, /result\.put\("phase", phase\)/);
  assert.match(javaSource, /result\.put\("synchronized", synchronizedSession\)/);
  assert.match(relayServiceSource, /invocationAdmission\.release\(\)/);
});

test("Android deadline expiry and later mutation admission share one atomic quarantine boundary", () => {
  const admission = javaSource.slice(
    javaSource.indexOf("private boolean beginRelayMutation"),
    javaSource.indexOf("private static RelayException relayDeadlineExpired"),
  );
  const expiry = javaSource.slice(
    javaSource.indexOf("private int expireRelayDeadline"),
    javaSource.indexOf("private JSObject usbDiagnostics()"),
  );
  const deadline = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayDeadline.java", import.meta.url), "utf8");
  const admissionGuardSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayAdmissionGuard.java", import.meta.url), "utf8");
  assert.match(admission, /relayAdmissionGuard\.beginMutation\(deadline == null \? null : deadline\.state\(\)\)/);
  assert.match(admissionGuardSource, /synchronized boolean beginMutation\(QcRelayDeadlineState deadline\)[\s\S]*!terminal && !writesQuarantined[\s\S]*deadline\.beginMutation\(\)/);
  assert.match(admissionGuardSource, /synchronized int expire\(QcRelayDeadlineState deadline\)[\s\S]*deadline\.expire\(\)[\s\S]*writesQuarantined = true;[\s\S]*writeQuarantineReason =/);
  assert.match(deadline, /session\.expireRelayDeadline\(this\)/);
  assert.match(deadline, /QcRelayDeadlineState state\(\) \{ return state; \}/);
  assert.doesNotMatch(deadline.slice(deadline.indexOf("void expire()"), deadline.indexOf("boolean isExpired()")), /completeExceptionally/,
    "the response timer must not release the service admission slot while native work is still owned");
  assert.match(javaSource, /operation\.whenComplete\(\(result, error\) -> \{[\s\S]*?invocation\.complete/);
  const serviceDispatch = relayServiceSource.slice(relayServiceSource.indexOf("private void dispatch("), relayServiceSource.indexOf("private JSONObject validatedResult"));
  assert.match(serviceDispatch, /scheduler\.schedule\(\(\) -> \{[\s\S]*expireRelayInvocation\(invocation\)/);
  assert.match(serviceDispatch, /invocation\s*\.whenComplete\(\(result, error\) -> \{[\s\S]*relayInvocationRetirement\(invocation\)[\s\S]*invocationAdmission\.release\(\)/);
  assert.match(serviceDispatch, /catch \(java\.util\.concurrent\.RejectedExecutionException error\)[\s\S]*expireRelayInvocation\(invocation\)[\s\S]*relayInvocationRetirement\(invocation\)[\s\S]*invocationAdmission\.release\(\)/);
  const trackedSubmit = javaSource.slice(javaSource.indexOf("private static void submitRelayTask"), javaSource.indexOf("private static CompletableFuture<org.json.JSONObject> failedRelay"));
  assert.match(trackedSubmit, /!deadline\.taskQueued\(\)\) return/);
  const trackedTask = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayTrackedTask.java", import.meta.url), "utf8");
  assert.match(trackedTask, /finally \{ retire\(\); \}/);
  assert.match(trackedTask, /void cancelBeforeRun\(\)[\s\S]*claimed\.compareAndSet\(false, true\)[\s\S]*retire\(\)/);
  assert.match(javaSource, /deadline\.retirement\(\)\.whenComplete\(\(ignored, error\) -> RELAY_DEADLINES\.remove/);
  const retirementModel = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayTaskRetirement.java", import.meta.url), "utf8");
  assert.match(retirementModel, /if \(operationFinished \|\| completion\.isDone\(\)\) return false/);
  const retirementTests = readFileSync(new URL("../android/app/src/test/java/org/freevia/qcremote/QcRelayTaskRetirementTest.java", import.meta.url), "utf8");
  assert.match(retirementTests, /queuedWorkKeepsRetirementPendingAfterOperationCompletes/);
  assert.match(retirementTests, /rejectedSubmissionRetiresItsReservation/);
  assert.match(retirementTests, /runningTaskRetainsSlotUntilItsFinallyRetires/);
  assert.match(retirementTests, /shutdownNowRetiresReturnedQueuedTaskAndRunningTaskExits/);
  assert.match(javaSource, /cancelDroppedRelayTasks\(commandIo\.shutdownNow\(\)\)/);
  assert.match(javaSource, /cancelDroppedRelayTasks\(midiIo\.shutdownNow\(\)\)/);
  assert.match(javaSource, /cancelRelayOperationsForSession\(this\)/);
  assert.match(deadline, /void cancelForShutdown\(\)[\s\S]*expire\(\);[\s\S]*current\.completeExceptionally/);
  assert.match(javaSource, /submitRelayTask\(commandIo, relayDeadline, \(\) -> \{[\s\S]*CURRENT_RELAY_DEADLINE\.set\(relayDeadline\)[\s\S]*relayInvoke\(method, deferredParams, deferredExpected\)/);
  for (const method of ["relayDisconnect", "relayReconnect", "issueBackupRequest", "executeRelayPlan", "relayGatewayReadOnCurrentSession", "relayMidi"])
    assert.match(javaSource, new RegExp(`${method}[\\s\\S]{0,3500}submitRelayTask\\(`), `${method} must retain its relay task until its executor runnable retires`);
  assert.match(javaSource, /private boolean beginRelayRead\(RelayDeadline deadline\)/);
  assert.match(javaSource, /relayGatewayReadWithRetry\(method, readParams, relayDeadline\)/);
  assert.match(javaSource, /relayGatewayReadOnCurrentSession\(method, readParams, scope, relayDeadline\)/);
  assert.match(javaSource, /if \(result\.isDone\(\)\) return;[\s\S]*?if \(relayDeadline != null && relayDeadline\.isExpired\(\)\) \{\s*result\.completeExceptionally\(relayDeadlineExpired\(\)\)/);
  assert.match(javaSource, /if \(!beginRelayRead\(relayDeadline\)\)[\s\S]*?result\.completeExceptionally\(relayDeadlineExpired\(\)\)/);
  const deadlineStateTest = readFileSync(new URL("../android/app/src/test/java/org/freevia/qcremote/QcRelayDeadlineStateTest.java", import.meta.url), "utf8");
  assert.match(deadlineStateTest, /mutationAdmissionAndExpiryHaveOneLinearizedOutcome/);
  assert.match(deadlineStateTest, /readAdmissionExpiresWithoutClassifyingTheOutcomeAsAMutation/);
  assert.match(deadlineStateTest, /assertEquals\(priorState == QcRelayDeadlineState\.MUTATION_ADMITTED, mutationAdmitted\)/);
  assert.match(javaSource, /private boolean beginRelayMutation/);
  assert.match(javaSource, /int expireRelayDeadline/);
  const guardTests = readFileSync(new URL("../android/app/src/test/java/org/freevia/qcremote/QcRelayAdmissionGuardTest.java", import.meta.url), "utf8");
  assert.match(admissionGuardSource, /synchronized boolean beginRead/);
  assert.match(admissionGuardSource, /synchronized int expire/);
  assert.match(admissionGuardSource, /synchronized void markTerminal/);
  assert.match(admissionGuardSource, /synchronized boolean publish/);
  assert.doesNotMatch(javaSource.slice(javaSource.indexOf("private boolean beginRelayMutation"), javaSource.indexOf("private static RelayException relayDeadlineExpired")), /synchronized/,
    "deadline admission/expiry must not acquire the monitor held across blocking HID writes");
  assert.match(javaSource, /relayAdmissionGuard\.markTerminal\(\)/,
    "destroy must mark the lifecycle terminal without acquiring the transport monitor");
  assert.match(guardTests, /expiryAndDestroyDoNotWaitForTransportMonitor/);
  assert.match(guardTests, /lifecyclePublicationAndDestroyShareOnlyTheShortGuard/);
});

test("critical Android flight-recorder events persist asynchronously with coalesced snapshots", () => {
  const flightRecorder = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcUsbFlightRecorder.java", import.meta.url), "utf8");
  const push = flightRecorder.slice(flightRecorder.indexOf('private void push('), flightRecorder.indexOf('private void persistAsync'));
  assert.match(push, /if \(critical\)[\s\S]*persistAsync\(snapshot\)/);
  assert.doesNotMatch(push, /synchronized \(fileLock\) \{ persist\(snapshot\); \}/);
  assert.match(flightRecorder, /AtomicReference<byte\[\]> pendingSnapshot/);
  assert.match(flightRecorder, /AtomicBoolean persistenceScheduled/);
});
