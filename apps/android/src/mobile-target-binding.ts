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

/** UI color/labels may describe a host demo as virtual without changing the
 * actual gateway route. Never install this presentation clone as app state. */
export function mobileIndicatorConnection(connection: import("@qc-remote/client").ConnectionState): import("@qc-remote/client").ConnectionState {
  return connection.brokerTarget?.source === "demo" ? { ...connection, source: "virtual" } : connection;
}

/** Adopt one explicit observation into its original provider. The callbacks
 * let the mobile controller retire old operations before accepting the new
 * snapshot, including a return to the same demo with a new broker generation. */
export async function refreshMobileTarget(options: {
  read(): Promise<{ connection: import("@qc-remote/client").ConnectionState; snapshot?: import("@qc-remote/client").PresetSnapshot }>;
  current(): boolean;
  origin: MobileProviderOrigin;
  provider: GatewayTransport;
  identity(connection: import("@qc-remote/client").ConnectionState, snapshot: import("@qc-remote/client").PresetSnapshot): string;
  reset(): void;
  select(binding: MobileTargetBinding): void;
  adopt(snapshot: import("@qc-remote/client").PresetSnapshot): Promise<unknown>;
  connection(value: import("@qc-remote/client").ConnectionState): void;
}): Promise<boolean> {
  const observation = await options.read();
  const assertCurrent = () => { if (!options.current()) throw new Error("The mobile target observation was superseded. Refresh again before editing."); };
  assertCurrent();
  const { connection, snapshot } = observation;
  if (connection.phase !== "ready" || !snapshot) {
    options.reset(); options.connection(connection); return false;
  }
  const binding = bindMobileTarget({ origin: options.origin, phase: connection.phase,
    kind: connection.brokerTarget?.source ?? (connection.source === "virtual" ? "demo" : "physical"),
    sessionIdentity: options.identity(connection, snapshot), providers: { [options.origin]: () => options.provider } })!;
  options.reset();
  options.select(binding);
  assertCurrent();
  await options.adopt(snapshot);
  assertCurrent();
  options.connection(connection);
  return true;
}
