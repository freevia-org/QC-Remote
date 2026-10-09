import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { HELP_WINDOW_TITLES, helpWindowKind } from "./about-content";
import { APP_WINDOW_TITLES, appWindowKind } from "./app-window-content";
import { QC_BRAND, QC_NATIVE_THEME } from "@qc-remote/theme";
import "@qc-remote/theme/theme.css";
import "./styles.css";

document.querySelector('meta[name="theme-color"]')?.setAttribute("content", QC_NATIVE_THEME.browser.windowsThemeColor);

// The main window loads index.html with no query; a Help window loads it as
// index.html?window=guide|feedback|about and an app window as
// index.html?window=settings|device-info|workspace, and each renders that page
// instead.
const helpKind = helpWindowKind(window.location.search);
const appKind = appWindowKind(window.location.search);
const root = createRoot(document.getElementById("root")!);

if (helpKind || appKind) {
  // Lets help-windows.css lift the main window's page rules (its 920px minimum
  // width and its hidden overflow) for this document only. The app windows
  // share the Help windows' page rules and reading-size type.
  document.documentElement.classList.add("qc-help-window");
}

if (helpKind) {
  document.title = HELP_WINDOW_TITLES[helpKind];
  void import("./help-window").then(({ HelpWindow }) => {
    root.render(
      <StrictMode>
        <HelpWindow kind={helpKind} />
      </StrictMode>
    );
  });
} else if (appKind) {
  document.title = APP_WINDOW_TITLES[appKind];
  void import("./app-window").then(({ AppWindow }) => {
    root.render(
      <StrictMode>
        <AppWindow kind={appKind} />
      </StrictMode>
    );
  });
} else {
  document.title = QC_BRAND.appName;
  root.render(
    <StrictMode>
      <App />
    </StrictMode>
  );
}
