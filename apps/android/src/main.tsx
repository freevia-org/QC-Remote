import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { StatusBar, Style } from "@capacitor/status-bar";
import { Capacitor } from "@capacitor/core";
import { App } from "./App";
import { QC_BRAND, QC_COLORS, QC_NATIVE_THEME } from "@qc-remote/theme";
import "@qc-remote/theme/theme.css";
import "./styles.css";

document.querySelector('meta[name="theme-color"]')?.setAttribute("content", QC_NATIVE_THEME.browser.androidThemeColor);
document.title = QC_BRAND.appName;

if (Capacitor.isNativePlatform()) {
  void StatusBar.setStyle({ style: Style.Dark });
  void StatusBar.setBackgroundColor({ color: QC_COLORS.app.canvas });
}

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
