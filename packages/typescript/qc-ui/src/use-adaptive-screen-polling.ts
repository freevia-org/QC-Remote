import { useEffect, useRef, useState } from "react";
import type { GatewayTransport } from "@qc-remote/client";

export const QC_SCREEN_FOLLOW_STORAGE_KEY = "qc-remote.follow-qc-screen-v1";

export type QcObservedScreenKind =
  | "grid"
  | "parameter-editor"
  | "tuner"
  | "tempo"
  | "gig"
  | "settings"
  | "io"
  | "directory"
  | "cpu-monitor"
  | "looper"
  | "unknown";

export type QcScreenObservation = {
  kind: QcObservedScreenKind;
  signature: string;
  observedAt: number;
};

export type QcScreenPollingPhase = "off" | "paused" | "polling" | "ready" | "error";

export function readQcScreenFollowSetting(storage: Pick<Storage, "getItem">): boolean {
  return storage.getItem(QC_SCREEN_FOLLOW_STORAGE_KEY) === "true";
}

export function writeQcScreenFollowSetting(storage: Pick<Storage, "setItem">, enabled: boolean): void {
  storage.setItem(QC_SCREEN_FOLLOW_STORAGE_KEY, String(enabled));
}

/**
 * Identify only screens whose owning class is unambiguous in the QC graphics
 * tree. Specific overlays must win over Grid and ParameterEditor because the
 * tree retains its parent screen underneath them.
 */
export function classifyQcGraphicsTree(tree: string): QcObservedScreenKind {
  const has = (name: string) => tree.includes(`zenUI::${name}`);
  if (has("TunerDialog")) return "tuner";
  if (has("MetronomeEditor")) return "tempo";
  if (has("GigView")) return "gig";
  if (has("SettingsDialog")) return "settings";
  if (has("CheatSheetDialog")) return "io";
  if (has("DirectoryDialog")) return "directory";
  if (has("CpuLoadIndicator")) return "cpu-monitor";
  if (has("Looper1TrackModelEditor")) return "looper";
  if (has("ParameterEditor")) return "parameter-editor";
  if (has("Grid")) return "grid";
  return "unknown";
}

/**
 * Dynamic labels, meters and parameter values change continuously. Class
 * identity and order describe the screen structure without mistaking those
 * live values for navigation.
 */
export function qcGraphicsTreeStructuralSignature(tree: string): string {
  return tree
    .split(/\r?\n/)
    .map((line) => line.match(/zenUI::[A-Za-z0-9_]+/)?.[0])
    .filter((value): value is string => Boolean(value))
    .join("\n");
}

/** Fast immediately after navigation, then progressively quieter while still. */
export function qcAdaptiveScreenPollDelay(stablePolls: number, consecutiveErrors = 0): number {
  if (consecutiveErrors > 0) return Math.min(5_000, 1_000 * 2 ** Math.min(consecutiveErrors, 3));
  if (stablePolls < 3) return 250;
  if (stablePolls < 10) return 500;
  return 1_000;
}

export function useAdaptiveQcScreenPolling({
  gateway,
  enabled,
  connected,
  onObservation
}: {
  gateway: Pick<GatewayTransport, "graphicsTree">;
  enabled: boolean;
  connected: boolean;
  onObservation: (observation: QcScreenObservation) => void;
}): { phase: QcScreenPollingPhase; observation?: QcScreenObservation } {
  const onObservationRef = useRef(onObservation);
  onObservationRef.current = onObservation;
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState !== "hidden");
  const [phase, setPhase] = useState<QcScreenPollingPhase>(enabled ? "paused" : "off");
  const [observation, setObservation] = useState<QcScreenObservation>();

  useEffect(() => {
    if (typeof document === "undefined") return;
    const update = () => setVisible(document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", update);
    window.addEventListener("pageshow", update);
    window.addEventListener("pagehide", update);
    return () => {
      document.removeEventListener("visibilitychange", update);
      window.removeEventListener("pageshow", update);
      window.removeEventListener("pagehide", update);
    };
  }, []);

  useEffect(() => {
    if (!enabled) { setPhase("off"); return; }
    if (!connected || !visible) { setPhase("paused"); return; }

    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let previousSignature: string | undefined;
    let stablePolls = 0;
    let consecutiveErrors = 0;

    const schedule = (delay: number) => {
      if (!cancelled) timer = setTimeout(() => void poll(), delay);
    };
    const poll = async () => {
      if (cancelled) return;
      setPhase((current) => current === "ready" ? current : "polling");
      try {
        const { tree } = await gateway.graphicsTree();
        if (cancelled) return;
        const signature = qcGraphicsTreeStructuralSignature(tree);
        const changed = signature !== previousSignature;
        stablePolls = changed ? 0 : stablePolls + 1;
        consecutiveErrors = 0;
        previousSignature = signature;
        if (changed) {
          const next = { kind: classifyQcGraphicsTree(tree), signature, observedAt: Date.now() } satisfies QcScreenObservation;
          setObservation(next);
          onObservationRef.current(next);
        }
        setPhase("ready");
        schedule(qcAdaptiveScreenPollDelay(stablePolls));
      } catch {
        if (cancelled) return;
        consecutiveErrors += 1;
        setPhase("error");
        schedule(qcAdaptiveScreenPollDelay(stablePolls, consecutiveErrors));
      }
    };

    void poll();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [connected, enabled, gateway, visible]);

  return { phase, observation };
}
