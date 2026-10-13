import { useEffect, useState } from "react";
import type { GatewayTransport } from "@qc-remote/client";
import { isUnitFrame } from "./qc-unit-screens.ts";
import "./unit-mirror.css";

/**
 * A screen the Quad Cortex shows that the app has no screen of its own for
 * (a confirmation, a keyboard, a picker): the unit's own picture, refreshed
 * while it is open. It is read-only. The app never confirms or dismisses
 * anything by itself; a tap the user makes on this picture is forwarded to the
 * same pixel on the unit (the display forwards it), and nothing else is sent.
 */
export function QcUnitScreenMirror({ gateway, intervalMs = 700 }: { gateway: Pick<GatewayTransport, "captureScreen">; intervalMs?: number }) {
  const [image, setImage] = useState<string>();
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const refresh = async () => {
      try {
        const frame = await gateway.captureScreen();
        if (!cancelled && isUnitFrame(frame.pngBase64)) setImage(`data:image/png;base64,${frame.pngBase64}`);
      } catch { /* the next refresh tries again */ }
      if (!cancelled) timer = setTimeout(() => void refresh(), intervalMs);
    };
    void refresh();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [gateway, intervalMs]);
  return <QcUnitPicture src={image} />;
}

/** The picture itself: the one element every captured-picture fallback draws, so one test can measure it. */
export function QcUnitPicture({ src }: { src?: string }) {
  return <section className="qc-screen coros-unit-mirror" aria-label="Quad Cortex screen, mirrored from the unit">
    {src ? <img src={src} alt="The screen the Quad Cortex is showing" draggable={false} /> : null}
  </section>;
}
