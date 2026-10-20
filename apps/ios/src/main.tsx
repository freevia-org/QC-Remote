import { StrictMode } from "react";
import * as React from "react";
import { createRoot } from "react-dom/client";
import { Capacitor } from "@capacitor/core";
import { StatusBar, Style } from "@capacitor/status-bar";
import { QC_BRAND, QC_COLORS, QC_NATIVE_THEME } from "@qc-remote/theme";
import { App } from "@qc-remote/android-src/App";
import { DeviceObservationProvider } from "@qc-remote/ui";
import { QCIosNativeBridge } from "../native-bridge";
import { IPhoneMidiController } from "./IPhoneMidiController";
import { IPadDriverStatus } from "./IPadDriverStatus";
import { getIosConnectionMode, setIosConnectionMode, remoteObservation, subscribeRemoteObservation, refreshRemoteObservation, type IosConnectionMode } from "./ios-services";
import "@qc-remote/theme/theme.css";
import "@qc-remote/android-src/styles.css";
import "./ios-midi.css";

type IosCapabilities = Awaited<ReturnType<typeof QCIosNativeBridge.platformCapabilities>>;

document.querySelector('meta[name="theme-color"]')?.setAttribute("content", QC_NATIVE_THEME.browser.androidThemeColor ?? QC_COLORS.app.canvas);
document.title = QC_BRAND.appName;

if (Capacitor.isNativePlatform()) void StatusBar.setStyle({ style: Style.Dark });

function ObservedApp({ remote }: { remote: boolean }) {
  const [lastObservedAt, setLastObservedAt] = React.useState(remoteObservation);
  React.useEffect(() => {
    let disposed = false;
    const subscription = subscribeRemoteObservation(value => { if (!disposed) setLastObservedAt(value); });
    return () => { disposed = true; void subscription.then(value => value.remove()); };
  }, []);
  return <DeviceObservationProvider value={remote
    ? { mode: "on-demand", lastObservedAt, refresh: refreshRemoteObservation }
    : { mode: "live" }}><App /></DeviceObservationProvider>;
}

function IosApp() {
  const [platform, setPlatform] = React.useState<(IosCapabilities & { family: "iphone" | "ipad" | "web" }) | null>(Capacitor.isNativePlatform() ? null : {
    family: "web", deviceFamily: "ipad", supportsUsbMidi: false, sharedProtocolRuntimeBundled: false, driverOsSupported: false,
    driverHardwareSupported: false,
    fullUsbDriverBundled: false, fullUsbReady: false, fullUsbRequirement: "", driverCompatibilityNote: "", driverEntitlementRequired: false
  });
  const [connectionMode, setConnectionMode] = React.useState<IosConnectionMode>(getIosConnectionMode() ?? "auto");
  const [modeSwitching, setModeSwitching] = React.useState(false);
  const [modeError, setModeError] = React.useState("");
  const modeSwitchInFlight = React.useRef(false);
  React.useEffect(() => {
    if (Capacitor.isNativePlatform()) void QCIosNativeBridge.platformCapabilities()
      .then((capabilities) => {
        setPlatform({ ...capabilities, family: capabilities.deviceFamily });
        const savedMode = getIosConnectionMode();
        setConnectionMode(savedMode === "remote" ? "remote" : capabilities.deviceFamily === "iphone" ? "midi" : "auto");
      })
      .catch(() => setPlatform({
        family: "iphone", deviceFamily: "iphone", supportsUsbMidi: true, sharedProtocolRuntimeBundled: false, driverHardwareSupported: false, driverOsSupported: false,
        fullUsbDriverBundled: false, fullUsbReady: false,
        fullUsbRequirement: "M-series iPad running iPadOS 16 or later with the QC Remote USB driver",
        driverCompatibilityNote: "Apple documents that DriverKit does not support USB devices that manipulate audio. Quad Cortex combines control HID with USB audio. This direct-control prototype is unapproved and unvalidated for release, and may interfere with QC audio or MIDI.",
        driverEntitlementRequired: true
      }));
  }, []);
  const changeConnectionMode = async (mode: IosConnectionMode) => {
    if (modeSwitchInFlight.current || mode === connectionMode) return;
    modeSwitchInFlight.current = true;
    setModeSwitching(true);
    setModeError("");
    try {
      await setIosConnectionMode(mode);
      setConnectionMode(mode);
    } catch (error) {
      setModeError(error instanceof Error ? error.message : "Could not change the connection route.");
    } finally {
      modeSwitchInFlight.current = false;
      setModeSwitching(false);
    }
  };
  if (platform === null) return <div className="ios-midi-loading" aria-label="Loading QC Remote" />;
  if (platform.family === "iphone") return <div className="ios-iphone-shell">
    <nav className="ios-connection-mode" aria-label="iPhone connection mode">
      <button type="button" disabled={modeSwitching} aria-pressed={connectionMode !== "remote"} onClick={() => void changeConnectionMode("midi")}>Direct USB MIDI</button>
      <button type="button" disabled={modeSwitching} aria-pressed={connectionMode === "remote"} onClick={() => void changeConnectionMode("remote")}>Paired host</button>
      {modeError && <p className="ios-mode-error" role="alert">{modeError}</p>}
    </nav>
    <div className={connectionMode === "remote" ? "ios-iphone-content is-remote" : "ios-iphone-content is-midi"}>
      {connectionMode === "remote" ? <ObservedApp key={connectionMode} remote={connectionMode === "remote"} /> : <IPhoneMidiController />}
    </div>
  </div>;
  if (platform.family === "ipad") return <IPadDriverStatus
    driverHardwareSupported={platform.driverHardwareSupported}
    driverOsSupported={platform.driverOsSupported}
    fullUsbDriverBundled={platform.fullUsbDriverBundled}
    fullUsbReady={platform.fullUsbReady}
    fullUsbRequirement={platform.fullUsbRequirement}
    driverCompatibilityNote={platform.driverCompatibilityNote}
    connectionMode={connectionMode === "remote" ? "remote" : "auto"}
    modeSwitching={modeSwitching}
    modeError={modeError}
    onConnectionModeChange={(mode) => void changeConnectionMode(mode)}>
    <ObservedApp key={connectionMode} remote={connectionMode === "remote"} />
  </IPadDriverStatus>;
  return <ObservedApp remote />;
}

createRoot(document.getElementById("root")!).render(<StrictMode><IosApp /></StrictMode>);
