import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { startRelayReportingFailure } from "./relay-start.ts";

test("actual Android prompt hook cancels replacement and unmount, and refuses retained old callbacks", async (t) => {
  const { build } = await import("esbuild");
  const { createRequire } = await import("node:module");
  const { mkdtempSync, rmSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const { fileURLToPath, pathToFileURL } = await import("node:url");
  const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
  const React = rendererRequire("react");
  const { act, create } = rendererRequire("react-test-renderer");
  const work = mkdtempSync(join(tmpdir(), "qc-android-prompts-"));
  t.after(() => rmSync(work, { recursive: true, force: true }));
  const outfile = join(work, "prompts.mjs");
  await build({
    entryPoints: [fileURLToPath(new URL("./use-android-prompts.tsx", import.meta.url))],
    outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
    plugins: [{ name: "prompt-test-components", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
      builder.onResolve({ filter: /^@qc-remote\/ui$/ }, () => ({ path: "ui", namespace: "prompt-test" }));
      builder.onLoad({ filter: /.*/, namespace: "prompt-test" }, () => ({ contents: "export const QcConfirm = () => null; export const QcTextPrompt = () => null;", loader: "js" }));
    } }]
  });
  const { useAndroidPrompts } = await import(pathToFileURL(outfile).href);
  let current: ReturnType<typeof useAndroidPrompts>;
  function Probe() { current = useAndroidPrompts(); return null; }
  let renderer: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  let first: Promise<boolean>;
  await act(async () => { first = current.prompts.confirm("Replace library?"); });
  const retiredConfirm = current.overlay.props.onConfirm;
  let second: Promise<string | null>;
  await act(async () => { second = current.prompts.prompt("Name preset", "Initial"); });
  assert.equal(await first!, false, "opening a text prompt cancels the older confirmation");
  await act(async () => retiredConfirm());
  assert.equal(current.overlay.props.title, "Name preset", "a retired dialog cannot hide or answer its successor");
  const retiredText = current.overlay.props.onSubmit;
  let third: Promise<boolean>;
  await act(async () => { third = current.prompts.confirm("Current confirmation"); });
  assert.equal(await second!, null, "opening a confirmation cancels the older text prompt");
  await act(async () => retiredText("Old result"));
  assert.equal(current.overlay.props.message, "Current confirmation");
  await act(async () => current.overlay.props.onConfirm());
  assert.equal(await third!, true);
  assert.equal(current.overlay, null);
  let pending: Promise<string | null>;
  await act(async () => { pending = current.prompts.prompt("Waiting on teardown", ""); });
  const retainedApi = current.prompts;
  const retainedSubmit = current.overlay.props.onSubmit;
  await act(async () => renderer.unmount());
  assert.equal(await pending!, null, "unmount settles the outstanding dialog promise");
  retainedSubmit("Too late");
  assert.equal(await retainedApi.confirm("After teardown"), false);
  assert.equal(await retainedApi.prompt("After teardown", ""), null);
});

test("the mobile control deck follows the QC layout in portrait and four-across landscape", () => {
  const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const performanceWorkflow = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-performance-workflow.ts", import.meta.url), "utf8");
  const destinationHook = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-screen-destination.ts", import.meta.url), "utf8");
  const deviceViews = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-device-views.ts", import.meta.url), "utf8");
  const displayModel = readFileSync(new URL("../../../packages/typescript/qc-ui/src/qc-device-display.tsx", import.meta.url), "utf8");
  const screenContent = readFileSync(new URL("../../../packages/typescript/qc-ui/src/qc-device-screen-content.tsx", import.meta.url), "utf8");
  const deviceSettingsScreen = readFileSync(new URL("../../../packages/typescript/qc-ui/src/device-settings-screen.tsx", import.meta.url), "utf8");
  const deviceBrowser = readFileSync(new URL("../../../packages/typescript/qc-ui/src/add-device-picker.tsx", import.meta.url), "utf8");
  const gridWorkflow = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-grid-workflow.ts", import.meta.url), "utf8");
  const surfaceSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/quad-cortex-surface.tsx", import.meta.url), "utf8");
  const surfaceStyles = readFileSync(new URL("../../../packages/typescript/qc-ui/src/surface-shell.css", import.meta.url), "utf8");
  const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
  const domain = JSON.parse(readFileSync(new URL("../../../contracts/public/qc-domain.v1.json", import.meta.url), "utf8"));

  assert.equal(domain.limits.scenes, 8);
  assert.match(appSource, /Array\.from\(\{ length: QC_SCENE_COUNT \}/, "A through H must come from the shared scene definition");
  assert.match(appSource, /QcMasterVolumeKnob value=\{snapshot\.masterVolume\} readout=\{`\$\{snapshot\.masterVolume\}`\}/);
  assert.match(appSource, /<small>v\{appPackage\.version\}<\/small>/, "the compact header identifies the app version instead of duplicating the QC preset title");
  assert.match(appSource, /openUsbStatusPanel/, "USB status opens a live details panel rather than immediately reconnecting");
  assert.match(appSource, /openRemoteStatusPanel/, "Remote status opens a live details panel rather than immediately unpairing");
  assert.match(appSource, /DEVICE CONNECTION[\s\S]*mobile-device-tabs/);
  assert.match(appSource, /workflowPanel === "remote" && <div className="mobile-connection-panel mobile-remote-panel">[\s\S]*?<h2 id="dialog-title">Quad Cortex pairing/);
  assert.match(appSource, /Remote service: \{relayWorkflow\.status\.endpoint\}/);
  assert.match(appSource, /mobile-status-notice/, "the compact control deck keeps operational feedback visible");
  assert.doesNotMatch(appSource, /mobile-chat|Chat settings|Assistant status/);
  assert.match(appSource, /The Quad Cortex is connected directly over USB/);
  assert.match(appSource, /ScreenWakeNative\.setEnabled\(\{ enabled \}\)/, "the foreground Android activity stays awake only for a live USB session");
  assert.match(appSource, /screenDimAfterMs = 90_000/);
  assert.match(appSource, /onPointerDown=\{resetScreenDimmer\}/);
  assert.match(styles, /\.android-app\.screen-dimmed::after \{ background: var\(--qc-palette-0008\); \}/);
  assert.match(appSource, /Keep screen awake while connected/);
  assert.match(deviceViews, /workflows\.ioEq\.refresh\(\)[\s\S]*destination\.setView\("io-overview"\)/, "I/O opens only after the unit's settings were read, through the one shared workflow");
  assert.match(appSource, /useQcDeviceViews\(\{[\s\S]*?destination: screenDestination/);
  assert.doesNotMatch(appSource, /QC_DEVICE_SCREEN_GESTURES/, "app-local edge gestures invoke semantic workflows rather than replaying screen coordinates");
  assert.doesNotMatch(appSource, /Switch to a connected Quad Cortex before opening (?:I\/O Settings|Gig View)/, "virtual-device screens open locally instead of requiring USB");
  assert.match(deviceViews, /workflows\.performance\.showDeviceView\(view, true\)[\s\S]*destination\.setView\(view\)/, "Gig View opens through the same command path as on Windows");
  assert.doesNotMatch(appSource, /androidGatewayTransport\.showGigView/, "no host-local Gig View command");
  assert.match(displayModel, /gigPresetList: preset\.presetList/);
  assert.match(displayModel, /onOpenGigView: \(\) => void views\.toggle\("gig"\), onOpenIoView|onOpenGigView: \(\) => void views\.toggle\("gig"\),\s+onOpenIoView: \(\) => void views\.toggle\("io"\)/);
  assert.match(displayModel, /sceneEditor: scene\.labelColorOpen \?/);
  assert.doesNotMatch(appSource, /mobile-scene-trigger/);
  assert.match(appSource, /mobile-device-view-control mobile-io-control[\s\S]*onClick=\{\(\) => void deviceViews\.toggle\("io"\)\}[\s\S]*>I\/O<\/button>/, "I/O remains reachable by an explicit control when a WebView gesture is interrupted");
  assert.match(appSource, /mobile-device-view-control mobile-gig-control[\s\S]*onClick=\{\(\) => void deviceViews\.toggle\("gig"\)\}[\s\S]*>GIG<\/button>/, "Gig View remains reachable by an explicit control when a WebView gesture is interrupted");
  assert.match(styles, /\.mobile-io-control \{ grid-column: 2; grid-row: 1; \}[\s\S]*\.mobile-gig-control \{ grid-column: 3; grid-row: 1; \}/, "I/O and GIG occupy the two open cells between Volume and Bank Up");
  assert.match(surfaceStyles, /\.qc-screen-bezel\.is-grid-edge-gesture-surface \{ touch-action: none; \}/, "the Android WebView cannot cancel QC edge swipes as page scrolling");
  assert.match(surfaceSource, /relativeY >= bounds\.height \* 0\.82[\s\S]*relativeY <= bounds\.height \* 0\.18[\s\S]*Math\.max\(24, swipe\.height \* 0\.08\)/, "edge swipe bands and travel remain usable on a short phone display");
  assert.doesNotMatch(appSource, /mobile-performance-control|workflowPanel === "performance"|>MORE</, "device screens stay on the QC navigation surface instead of an invented app dialog");
  assert.match(appSource, /onDoubleTap=\{\(\) => openTempoOrTuner\("double-tap"\)\}/);
  assert.match(appSource, /onHold=\{\(\) => openTempoOrTuner\("hold"\)\}/);
  assert.match(appSource, /swapTempoTunerAccess/, "TEMPO hold/double-tap follows the device setting");
  assert.match(appSource, /handleModeChordContact\("bankDown", pressed\)/);
  assert.match(appSource, /handleModeChordContact\("tempo", pressed\)/);
  assert.match(appSource, /gigViewStompAccess/, "BANK DOWN + TEMPO hold respects the device's Gig View setting");
  assert.match(appSource, /void deviceSettingsWorkflow\.refresh\(\)/, "gesture settings are read from the connected QC before use");
  assert.match(displayModel, /onOpenIoView: \(\) => void views\.toggle\("io"\)/, "the QC top-edge swipe toggles I\/O Settings");
  assert.match(appSource, /mobileScreenView === "tuner"\) void deviceViews\.close\(\)/, "BANK UP exits Tuner through the QC control");
  assert.match(appSource, /tempoWorkflow\.open\) tempoWorkflow\.close\(\)/, "BANK UP exits Tempo through the QC control");
  assert.match(appSource, /\bLOOPER\b|\\blooper\\b/i, "Looper X opens from its Grid block");
  assert.match(deviceViews, /gateway\.longTapScreen\(x, y\)/, "device-screen holds reach the physical QC as long taps");
  assert.match(displayModel, /views\.forwardGesture\(x, y, toX, toY, kind\)/);
  assert.match(deviceBrowser, /onTogglePinned\(modelId\)/, "holding a model uses the QC's pin gesture");
  assert.match(deviceBrowser, /consumedModelTap\.current === model\.id/, "a model hold cannot also place the model");
  assert.match(gridWorkflow, /gateway\.setModelPinned\(modelId, !expectedPinned, expectedPinned\)/);
  assert.match(gridWorkflow, /confirmed\.models\.includes\(modelId\) === expectedPinned/, "model pinning is confirmed by readback");
  assert.match(destinationHook, /deviceSettings\.openSettings\(returnFocusTarget\)/, "Grid Settings opens device settings, not the Remote panel");
  assert.match(appSource, /createQcDeviceDisplay\(\{[\s\S]*?deviceSettings: deviceSettingsWorkflow/);
  assert.match(screenContent, /<CorOsDeviceSettings workflow=\{workflows\.deviceSettings\}/);
  assert.match(deviceSettingsScreen, /Support[\s\S]*Device Information/);
  assert.match(deviceSettingsScreen, /<CorOsReferenceSettingsPane view="settings-info" deviceInformation=\{info\} \/>/, "Device Information is the shared pane fed the unit's real values");
  assert.match(appSource, /createQcDeviceDisplay\(\{[\s\S]*?tempo: tempoWorkflow/);
  assert.match(screenContent, /<CorOsTempoScreen workflow=\{workflows\.tempo\}/);
  assert.match(appSource, /QcHardwareSwitch role="bank:up" label=\{<QcUiIcon kind="up" \/>\}/);
  assert.match(appSource, /QcHardwareSwitch role="bank:down" label=\{<span className="mobile-down-glyph"><QcUiIcon kind="up" \/><\/span>\}/);
  assert.doesNotMatch(appSource, />SCENE</);
  assert.match(appSource, /footswitchLeds\(snapshot\)/);
  assert.match(appSource, /useQcWorkflows\(\{/);
  assert.match(performanceWorkflow, /controller\.beginFootswitch/);
  assert.match(appSource, /QcHardwareSwitch role="tempo" label="" ariaLabel="TEMPO" readout=\{`\$\{snapshot\.tempo\}`\}/);
  assert.match(appSource, /useContinuousControlWorkflow\(\{/);
  assert.match(appSource, /rotation: \{ editor: adjustEditorParameter, tempo: continuousControls\.adjustTempo, masterVolume: continuousControls\.adjustMasterVolume \}/, "encoder turns reach the editor first, then Tempo and Master Volume, through the shared router");
  assert.doesNotMatch(appSource, /> BLOCK<\/button>/);
  assert.doesNotMatch(appSource, />EDIT BLOCK<\/button>/);
  assert.doesNotMatch(appSource, />REDO<\/button>/);
  assert.doesNotMatch(appSource, /className="workflow-actions"/);
  assert.doesNotMatch(appSource, />ROUTING<\/button>/);
  assert.doesNotMatch(appSource, />SCENES<\/button>/);
  assert.match(styles, /grid-template-columns: repeat\(5, minmax\(0, 1fr\)\)/);
  assert.match(styles, /grid-template-rows: repeat\(3, minmax\(66px, 1fr\)\)/);
  assert.match(styles, /\.quick-controls \{[^}]*overscroll-behavior: contain; touch-action: none;/, "vertical encoder drags must belong to the controls instead of panning the Android WebView");
  assert.match(styles, /html, body, #root \{[^}]*overscroll-behavior: none;/, "the fixed Android shell must not rubber-band during a hardware-control drag");
  assert.match(styles, /\.quick-controls \.switch-ring \{ width: 50px;/);
  assert.match(styles, /\.quick-controls \.switch-label \{ position: relative; z-index: 5; grid-column: 1; grid-row: 2; place-self: center; left: auto; top: auto; transform: none;/);
  assert.match(styles, /\.quick-controls \.switch-ring \{ grid-column: 1; grid-row: 2; \}/, "labels and arrows share the ring's grid cell instead of using a device-specific pixel offset");
  assert.match(styles, /\.quick-controls \.switch-led \{ position: relative; width: 10px; align-self: end; margin-bottom: 3px;/);
  assert.match(styles, /\.quick-controls \.switch-ring \{ width: 50px;/);
  assert.match(styles, /\.mobile-up-control \{ grid-column: 5; grid-row: 1; \}/);
  assert.match(styles, /\.mobile-down-control \{ grid-column: 5; grid-row: 2; \}/);
  assert.match(styles, /\.mobile-tempo-control \{ grid-column: 5; grid-row: 3; \}/);
  assert.match(styles, /\.hardware-switch\.is-tempo-pulse\.is-active \.switch-led::before/);
  assert.match(styles, /\.mobile-down-glyph \{ display: inline-grid; transform: rotate\(180deg\); \}/);
  assert.match(styles, /\.mobile-up-control \.switch-label svg,[\s\S]*width: 17px; height: 17px;/);
  const rotaryGesture = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-rotary-gesture.ts", import.meta.url), "utf8");
  assert.match(rotaryGesture, /export const hardwareRotaryPixelsPerStep = \(pointerType: string\) => pointerType === "touch" \? 6 : 12;/, "touch encoders use a responsive threshold while mouse behavior stays unchanged");
  assert.equal(surfaceSource.match(/pixelsPerStep: hardwareRotaryPixelsPerStep/g)?.length, 2, "both chassis encoders and the master volume share it");
  assert.match(styles, /\.mobile-volume-control \{ grid-column: 1; grid-row: 1;/);
  assert.match(styles, /@media \(orientation: landscape\)[\s\S]*grid-template-columns: repeat\(4, minmax\(0, 1fr\)\)/);
  assert.match(styles, /@media \(orientation: landscape\)[\s\S]*\.mobile-screen \{[^}]*container-type: size; --qc-display-max-height: 100cqh; \}/, "landscape only changes the box the display may fill; the display frame keeps its own 5:3 shape");
  assert.match(appSource, /<QcDeviceDisplay display=\{display\} presentation="display" /, "the phone shows the shared display frame without the hardware panel");
});

test("tapping a live Grid block opens the shared parameter editor and commits over USB", () => {
  const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const displayModel = readFileSync(new URL("../../../packages/typescript/qc-ui/src/qc-device-display.tsx", import.meta.url), "utf8");
  const gridWorkflow = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-grid-workflow.ts", import.meta.url), "utf8");
  const parameterWorkflow = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-parameter-workflow.ts", import.meta.url), "utf8");
  const parameterBindings = readFileSync(new URL("../../../packages/typescript/qc-ui/src/qc-parameter-editor-bindings.ts", import.meta.url), "utf8");
  const surfaceActions = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-surface-actions.ts", import.meta.url), "utf8");
  const nativeSource = readFileSync(new URL("./native-services.ts", import.meta.url), "utf8");
  const coreSource = readFileSync(new URL("../../../packages/typescript/qc-core/src/state.ts", import.meta.url), "utf8");
  const liveStateSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/use-qc-live-state.ts", import.meta.url), "utf8");
  const styles = readFileSync(new URL("./styles.css", import.meta.url), "utf8");

  assert.match(gridWorkflow, /gateway\.blockDetails\(block\.row, block\.column, snapshot\.presetName, requirePresetIdentity\(snapshot\)\)/);
  assert.match(appSource, /parameterEditor: parameterEditorBindings/);
  assert.match(displayModel, /hardwareLeds: qcParameterEditorLeds\(host\.parameterEditor\)/, "the phone's switches light from the display's own LED derivation");
  assert.match(parameterBindings, /const details = editor\.details;[\s\S]*if \(!details\) return undefined/);
  assert.match(parameterWorkflow, /operationGateway\.setParameter\(row, column, parameter\.index, value/);
  assert.match(nativeSource, /createAndroidQcTransport[\s\S]*createQcGatewayTransport/);
  assert.doesNotMatch(nativeSource, /QcUsbNative\.(?:blockDetails|setParameter)/);
  assert.match(appSource, /useQcSurfaceActions\(\{/);
  assert.match(surfaceActions, /blockSelectionIntent\(selectedBlockId, blockId\)/);
  assert.match(appSource, /consumeQcNativeStateFrame\(frame/);
  assert.match(liveStateSource, /reconcileFrame\(states, observedAt\)/);
  assert.match(coreSource, /dirty: state\.dirty \?\? snapshot\.dirty/);
  assert.doesNotMatch(styles, /preset-title/, "the live Grid's preset title is drawn by the shared display and never restyled per host");
});

test("Android exposes shared About, privacy, legal, and third-party notices", () => {
  const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const themeSource = readFileSync(new URL("../../../packages/typescript/qc-theme/src/legal.ts", import.meta.url), "utf8");

  assert.match(appSource, /QC_LEGAL/);
  assert.match(appSource, /About \$\{QC_BRAND\.appName\}/);
  for (const panel of ["about", "privacy", "legal", "notices"]) {
    assert.match(appSource, new RegExp(`workflowPanel === "${panel}"`));
  }
  assert.match(themeSource, /not affiliated with, authorized, sponsored, endorsed, or supported/);
  assert.match(themeSource, /identify the product with which this application is compatible/);
  // The in-app Privacy view is a pointer to PRIVACY.md and carries no facts of
  // its own, so it cannot drift from the policy.
  assert.match(themeSource, /privacy: \{\s*(?:\/\/[^\n]*\n\s*)*pointer: "This screen only points to the privacy policy, the one authoritative text\./);
  assert.doesNotMatch(themeSource, /operated by Freevia EOOD|can access routed commands|does not include advertising or analytics/);
  assert.match(appSource, /QC_LEGAL\.privacy\.pointer/);
  assert.match(appSource, /QC_BRAND\.privacyPolicyUrl/);
  assert.doesNotMatch(appSource, /GeminiNative|VoiceInputNative|mobile-chat/);
});

test("MCP and relay access defaults to full control and enforces four tiers", () => {
  const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const servicesSource = readFileSync(new URL("./native-services.ts", import.meta.url), "utf8");
  const accessSelectSource = readFileSync(new URL("../../../packages/typescript/qc-ui/src/control-access-select.tsx", import.meta.url), "utf8");
  const relaySource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/QcRelayService.java", import.meta.url), "utf8");
  const policySource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayAccessPolicy.java", import.meta.url), "utf8");

  assert.match(appSource, /readControlAccessMode\(window\.localStorage, \[legacyControlAccessModeKey\]\)/);
  assert.match(appSource, /writeControlAccessMode\(window\.localStorage, mode\)/);
  assert.match(appSource, /ariaLabel="MCP device access"/);
  assert.match(appSource, /relayWorkflow\.setAccessMode\(mode\)/);
  assert.match(accessSelectSource, /SHARED_QC_ACCESS_MODES\.map\(\(value\) => \(\{ value, label: accessLabels\[value\] \}\)\)/);
  assert.match(accessSelectSource, /performance: "Performance", modify: "Modify"/);
  assert.match(accessSelectSource, /"read-only": "Read-only"/);
  assert.match(servicesSource, /type PublicRelayState, type PublicRelayStatus/);
  assert.match(servicesSource, /setAccessMode\(options: \{ mode: ControlAccessMode \}\)/);
  assert.match(servicesSource, /publicRelay: PublicRelayPort/);
  assert.match(appSource, /publicRelay\.browserPairing!\.begin\(\)/);
  assert.match(appSource, /publicRelay\.browserPairing!\.poll\(\)/);
  const pairingPollSource = readFileSync(new URL("./browser-pairing-poll.ts", import.meta.url), "utf8");
  assert.match(appSource, /createBrowserPairingPoll\(\{/);
  assert.match(pairingPollSource, /!disposed && result\.state === "paired"/);
  assert.doesNotMatch(appSource, /result\.paired/);
  assert.doesNotMatch(appSource, /QcRelayNative\.(beginBrowserPairing|pollBrowserPairing)/);
  assert.match(servicesSource, /beginBrowserPairing\(\{ endpoint: QC_BRAND\.publicRelayEndpoint \}\)/);
  assert.match(servicesSource, /pollBrowserPairing\(\)/);
  assert.match(servicesSource, /result\.state === "paired" \|\| result\.paired === true/);
  assert.doesNotMatch(appSource, /window\.prompt\("One-time pairing code"\)/);
  assert.match(appSource, /usePublicRelayWorkflow\(\{ relay: publicRelay/);
  assert.match(policySource, /getString\(MODE, FULL\)/);
  assert.match(policySource, /GeneratedRemoteActions\.isPerformance/);
  assert.match(policySource, /GeneratedRemoteActions\.isModify/);
  assert.match(relaySource, /RelayAccessPolicy\.permits/);
  const frameSource = readFileSync(new URL("../android/app/src/main/java/org/freevia/qcremote/RelayFrameHandler.java", import.meta.url), "utf8");
  assert.match(relaySource, /relayFrameHandler\.inspect\(text/);
  assert.match(frameSource, /"ACCESS_MODE_RESTRICTED"/);
});
test("a rejected Remote reconnect surfaces the plugin message and code as a notice", async () => {
  const notices: string[] = [];
  const rejection = Object.assign(new Error("Connect the Quad Cortex and grant USB access before starting Remote."), { code: "USB_PERMISSION_REQUIRED" });
  await startRelayReportingFailure(() => Promise.reject(rejection), (text) => notices.push(text));
  assert.deepEqual(notices, ["Connect the Quad Cortex and grant USB access before starting Remote. (USB_PERMISSION_REQUIRED)"]);
  const appSource = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  assert.match(appSource, /startRelayReportingFailure\(relayWorkflow\.start, setNotice\)/, "the Reconnect action must not drop QcRelayPlugin.start rejections");
  assert.doesNotMatch(appSource, /void \(?[^;]*\brelayWorkflow\.start\(\)/);
});


test("Android replacement clears previous-session suspension and bulk blocking while current publications still block", async (t) => {
  const { build } = await import("esbuild");
  const { createRequire } = await import("node:module");
  const { mkdtempSync, rmSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const { fileURLToPath, pathToFileURL } = await import("node:url");
  const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
  const React = rendererRequire("react");
  const { act, create } = rendererRequire("react-test-renderer");
  const work = mkdtempSync(join(tmpdir(), "qc-android-blocks-"));
  t.after(() => rmSync(work, { recursive: true, force: true }));
  const outfile = join(work, "blocks.mjs");
  await build({
    entryPoints: [fileURLToPath(new URL("./use-android-session-blocks.ts", import.meta.url))],
    outfile, bundle: true, format: "esm", platform: "node", logLevel: "error",
    plugins: [{ name: "test-react", setup(builder) {
      builder.onResolve({ filter: /^react$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
    } }]
  });
  const { useAndroidSessionBlocks } = await import(pathToFileURL(outfile).href);
  let state: ReturnType<typeof useAndroidSessionBlocks>;
  function Probe() { state = useAndroidSessionBlocks(); return null; }
  let renderer: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  await act(async () => { state.setDeviceSuspended(true); state.setDeviceBulkBlocking(true); });
  assert.equal(state.deviceSuspended || state.deviceBulkBlocking, true);
  await act(async () => state.resetSessionBlocks());
  assert.equal(state.deviceSuspended || state.deviceBulkBlocking, false, "the initial new stream needs no old-session finish event");
  await act(async () => { state.resetSessionBlocks(); state.setDeviceBulkBlocking(true); });
  assert.equal(state.deviceBulkBlocking, true, "a blocking publication arriving during the new handshake is preserved");
  await act(async () => state.setDeviceBulkBlocking(false));
  assert.equal(state.deviceBulkBlocking, false, "the current operation's terminal event restores controls");
  await act(async () => state.setDeviceSuspended(true));
  assert.equal(state.deviceSuspended, true, "a genuine current suspension remains effective");
});
