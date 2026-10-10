import { lazy, Suspense } from "react";
import { IDLE_PLACEHOLDER_SNAPSHOT } from "./device-placeholder";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane";
import "./qc-device-typography.css";
import "./display-frame.css";

const CorOsScreenFixture = lazy(() => import("./coros-screen-fixtures").then((module) => ({ default: module.CorOsScreenFixture })));

/**
 * The unit's Settings > Support > Device Information screen on its own, with
 * no panel around it: the same display frame, canvas and screen the main
 * window draws (display-frame.css), filling the width its container offers.
 * A window that shows one screen uses this instead of drawing the screen
 * itself, so it can never differ from the display on the main surface.
 */
export function QcDeviceInformationDisplay({ deviceInformation, onClose }: { deviceInformation: CorOsDeviceInformation; onClose: () => void }) {
  return <div className="qc-display-standalone">
    <div className="qc-screen-bezel">
      <div className="qc-screen-fixture-root">
        <Suspense fallback={null}><CorOsScreenFixture view="settings-info" snapshot={IDLE_PLACEHOLDER_SNAPSHOT} deviceInformation={deviceInformation} onClose={onClose} /></Suspense>
      </div>
    </div>
  </div>;
}
