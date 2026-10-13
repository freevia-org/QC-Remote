import { StrictMode } from "react";
import * as React from "react";
import { createRoot } from "react-dom/client";
import { Capacitor } from "@capacitor/core";
import { StatusBar, Style } from "@capacitor/status-bar";
import { QC_BRAND, QC_COLORS, QC_NATIVE_THEME } from "@qc-remote/theme";
import { App } from "@qc-remote/android-src/App";
import { QCIosNativeBridge } from "../native-bridge";
import { IPhoneMidiController } from "./IPhoneMidiController";
import { IPadDriverStatus } from "./IPadDriverStatus";
import "@qc-remote/theme/theme.css";
import "@qc-remote/android-src/styles.css";
import "./ios-midi.css";

type IosCapabilities = Awaited<ReturnType<typeof QCIosNativeBridge.platformCapabilities>>;

document.querySelector('meta[name="theme-color"]')?.setAttribute("content", QC_NATIVE_THEME.browser.androidThemeColor ?? QC_COLORS.app.canvas);
document.title = QC_BRAND.appName;

if (Capacitor.isNativePlatform()) void StatusBar.setStyle({ style: Style.Dark });

function IosApp() {
  const [platform, setPlatform] = React.useState<(IosCapabilities & { family: "iphone" | "ipad" | "web" }) | null>(Capacitor.isNativePlatform() ? null : {
    family: "web", deviceFamily: "ipad", supportsUsbMidi: false, sharedProtocolRuntimeBundled: false, driverOsSupported: false,
    driverHardwareSupported: false,
    fullUsbDriverBundled: false, fullUsbReady: false, fullUsbRequirement: "", driverCompatibilityNote: "", driverEntitlementRequired: false
  });
  React.useEffect(() => {
    if (Capacitor.isNativePlatform()) void QCIosNativeBridge.platformCapabilities()
      .then((capabilities) => setPlatform({ ...capabilities, family: capabilities.deviceFamily }))
      .catch(() => setPlatform({
        family: "iphone", deviceFamily: "iphone", supportsUsbMidi: true, sharedProtocolRuntimeBundled: false, driverHardwareSupported: false, driverOsSupported: false,
        fullUsbDriverBundled: false, fullUsbReady: false,
        fullUsbRequirement: "M-series iPad running iPadOS 16 or later with the QC Remote USB driver",
        driverCompatibilityNote: "Apple documents that DriverKit does not support USB devices that manipulate audio. Quad Cortex combines control HID with USB audio. This direct-control prototype is unapproved and unvalidated for release, and may interfere with QC audio or MIDI.",
        driverEntitlementRequired: true
      }));
  }, []);
  if (platform === null) return <div className="ios-midi-loading" aria-label="Loading QC Remote" />;
  if (platform.family === "iphone") return <IPhoneMidiController />;
  if (platform.family === "ipad") return <IPadDriverStatus driverHardwareSupported={platform.driverHardwareSupported} driverOsSupported={platform.driverOsSupported} fullUsbDriverBundled={platform.fullUsbDriverBundled} fullUsbReady={platform.fullUsbReady} fullUsbRequirement={platform.fullUsbRequirement} driverCompatibilityNote={platform.driverCompatibilityNote}><App /></IPadDriverStatus>;
  return <App />;
}

createRoot(document.getElementById("root")!).render(<StrictMode><IosApp /></StrictMode>);
