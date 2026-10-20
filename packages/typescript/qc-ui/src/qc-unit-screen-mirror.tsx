import { useDeviceObservationPolicy } from "./device-observation-policy.ts";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { GatewayTransport } from "@qc-remote/client";
import type { PresetLifetime, PresetOperation } from "@qc-remote/core";
import { captureGatewaySession } from "./gateway-session.ts";
import { isUnitFrame } from "./qc-unit-screens.ts";
import "./unit-mirror.css";

/**
 * A screen the Quad Cortex shows that the app has no screen of its own for
 * (a confirmation, a keyboard, a picker): the unit's own picture, refreshed
 * while it is open. It is read-only. The app never confirms or dismisses
 * anything by itself; a tap the user makes on this picture is forwarded to the
 * same pixel on the unit (the display forwards it), and nothing else is sent.
 */
export function QcUnitScreenMirror({ gateway, lifetime, intervalMs = 700, onReturnToGrid, captureSessionCurrent }: { gateway: Pick<GatewayTransport, "captureScreen">; lifetime?: PresetLifetime; intervalMs?: number; onReturnToGrid?(): void; captureSessionCurrent?(): () => boolean }) {
  const passivePolling = useDeviceObservationPolicy().mode === "live";
  const [refreshRevision, setRefreshRevision] = useState(0);
  const handledRefresh = useRef(0);
  const [image, setImage] = useState<{ gateway: typeof gateway; origin?: PresetOperation; current(): boolean; value?: string; status: "loading" | "ready" | "error" }>();
  const mounted = useRef(true);
  // Hosts build this probe factory inline, so its identity changes on every host render. It is read through a ref: as an
  // effect dependency it restarted the capture loop (cancelling the in-flight capture) on each host render, and a host that
  // renders more often than the unit answers never showed a picture.
  const hostProbe = useRef(captureSessionCurrent);
  hostProbe.current = captureSessionCurrent;
  const returnOrigin = lifetime?.checkpoint();
  const returnHostCurrent = captureSessionCurrent?.() ?? (() => true);
  const returnSession = captureGatewaySession(gateway as GatewayTransport, () => mounted.current && returnHostCurrent() && (!returnOrigin || lifetime!.currentSession(returnOrigin)));
  useEffect(() => {
    mounted.current = true;
    if (!passivePolling && refreshRevision === handledRefresh.current) return;
    if (!passivePolling) handledRefresh.current = refreshRevision;
    let disposed = false;
    let generation = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let origin = lifetime?.checkpoint();
    const restart = () => {
      clearTimeout(timer);
      timer = undefined;
      const token = ++generation;
      origin = lifetime?.checkpoint();
      const capturedOrigin = origin;
      const hostCurrent = hostProbe.current?.() ?? (() => true);
      const session = captureGatewaySession(gateway as GatewayTransport, () => !disposed && token === generation
        && hostCurrent() && (!capturedOrigin || lifetime!.currentSession(capturedOrigin)));
      setImage({ gateway, origin: capturedOrigin, current: session.current, status: "loading" });
      const refresh = async () => {
        try {
          const frame = await session.gateway.captureScreen();
          if (session.current() && isUnitFrame(frame.pngBase64)) setImage({ gateway, origin: capturedOrigin, current: session.current, value: `data:image/png;base64,${frame.pngBase64}`, status: "ready" });
        } catch {
          if (session.current()) setImage({ gateway, origin: capturedOrigin, current: session.current, status: "error" });
        }
        if (passivePolling && session.current()) timer = setTimeout(() => void refresh(), intervalMs);
      };
      void refresh();
    };
    const unsubscribe = passivePolling ? lifetime?.subscribe(() => {
      if (origin && !lifetime.currentSession(origin)) restart();
    }) : undefined;
    restart();
    return () => { mounted.current = false; disposed = true; generation++; clearTimeout(timer); unsubscribe?.(); };
  }, [gateway, lifetime, intervalMs, passivePolling, refreshRevision]);
  const currentImage = image?.gateway === gateway && image.current() ? image : undefined;
  return <><QcUnitPicture src={currentImage?.value}>
    {!currentImage?.value && <div className="unit-mirror-status" role="status">
      <p>{currentImage?.status === "error" ? "The device screen could not be read." : !passivePolling && refreshRevision === 0 ? "Refresh to read the device screen." : "Reading the device screen…"}</p>
      {onReturnToGrid && <button type="button" onPointerDown={(event) => event.stopPropagation()} onPointerUp={(event) => event.stopPropagation()} onPointerCancel={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); if (returnSession.current()) onReturnToGrid(); }}>Return to Grid</button>}
    </div>}
  </QcUnitPicture>{!passivePolling && <button type="button" onClick={() => setRefreshRevision(value => value + 1)}>Refresh screen</button>}</>;
}

/** One element for every captured-picture fallback, including the loading/error state. */
export function QcUnitPicture({ src, children }: { src?: string; children?: ReactNode }) {
  return <section className="qc-screen coros-unit-mirror" aria-label="Quad Cortex screen, mirrored from the unit">
    {src ? <img src={src} alt="The screen the Quad Cortex is showing" draggable={false} /> : children}
  </section>;
}
