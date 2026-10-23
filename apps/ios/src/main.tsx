import { StrictMode } from "react";
import * as React from "react";
import { createRoot } from "react-dom/client";
import { Capacitor } from "@capacitor/core";
import { StatusBar, Style } from "@capacitor/status-bar";
import { QC_BRAND, QC_COLORS, QC_NATIVE_THEME } from "@qc-remote/theme";
import { App } from "@qc-remote/android-src/App";
import { DeviceObservationProvider } from "@qc-remote/ui";
import { remoteObservation, subscribeRemoteObservation, refreshRemoteObservation, refreshRemoteTargetObservation, remoteConnection, clearRemoteTarget } from "./ios-services";
import "@qc-remote/theme/theme.css";
import "@qc-remote/android-src/styles.css";
import "./ios-midi.css";


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
    ? { mode: "on-demand", lastObservedAt, refresh: refreshRemoteObservation, readTarget: refreshRemoteTargetObservation, connection: remoteConnection, clearTarget: clearRemoteTarget }
    : { mode: "live" }}><App /></DeviceObservationProvider>;
}

function IosApp() {
  return <div className="ios-iphone-shell">
    <aside className="ios-paired-host-notice" aria-label="Paired-host connection">
      <strong>Paired host</strong>
      <span>Connect your Quad Cortex by USB to a computer running QC Remote. Sign in with the same account to control it here.</span>
    </aside>
    <div className="ios-iphone-content is-remote"><ObservedApp remote /></div>
  </div>;
}

createRoot(document.getElementById("root")!).render(<StrictMode><IosApp /></StrictMode>);
