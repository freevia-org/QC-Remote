import type { ConnectionPhase, GatewayTransport } from "@qc-remote/client";

/** App-internal provider identity, separate from the selected target kind. */
export type MobileProviderOrigin = "local-demo" | "paired-host" | "native-gateway";
export type MobileTargetKind = "physical" | "demo";
export interface MobileTargetBinding {
  origin: MobileProviderOrigin;
  kind: MobileTargetKind;
  /** Opaque provider session identity; changes must fence captured operations. */
  key: string;
  gateway: GatewayTransport;
}

/** Bind an already-observed ready target. This does no I/O, authentication,
 * discovery, demo loading or fallback selection. Physical-first selection is
 * the serving provider's responsibility, not a guess from ConnectionState.demo.
 * This is an app-internal normalized model, not a new MCP wire contract. */
export function bindMobileTarget(options: {
  origin: MobileProviderOrigin;
  phase: ConnectionPhase;
  kind: MobileTargetKind;
  sessionIdentity: string;
  providers: Partial<Record<MobileProviderOrigin, () => GatewayTransport | undefined>>;
}): MobileTargetBinding | undefined {
  const { origin, phase, kind, sessionIdentity, providers } = options;
  if (phase !== "ready") return undefined;
  if (!sessionIdentity) throw new Error("The selected device session identity is unavailable. Refresh before editing.");
  if (kind !== "physical" && kind !== "demo") throw new Error("The selected device kind is unavailable. Refresh before editing.");
  if (origin === "local-demo" && kind !== "demo") throw new Error("A local demo cannot represent physical hardware.");
  const gateway = providers[origin]?.();
  if (!gateway) throw new Error("The selected device provider is unavailable. Refresh before editing.");
  return { origin, kind, key: JSON.stringify([origin, kind, sessionIdentity]), gateway };
}
