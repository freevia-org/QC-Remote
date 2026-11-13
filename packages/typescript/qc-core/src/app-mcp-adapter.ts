import { brokerTargetKey, selectedBrokerTarget, GATEWAY_CLIENT_METHODS, type BrokerTarget } from "@qc-remote/client";
import { controlAccessPermitsAction, type ControlAccessMode } from "./control-access.ts";
import { SHARED_QC_ACTIONS } from "./generated-actions.ts";

// Built once: the catalogue is large and invokeAppMcp runs on every gateway call.
const GATEWAY_RPCS: ReadonlySet<string> = new Set(Object.values(GATEWAY_CLIENT_METHODS).map((entry) => entry.rpc));
const ACTION_BY_RPC = new Map<string, (typeof SHARED_QC_ACTIONS)[number]>();
for (const action of SHARED_QC_ACTIONS) if (!ACTION_BY_RPC.has(action.rpc)) ACTION_BY_RPC.set(action.rpc, action);

/** Dedicated read tools are deliberately separate from execute_qc_action. */
export const APP_MCP_READ_TOOLS: Readonly<Record<string, string>> = {
  "system.status": "get_qc_status",
  "device.snapshot": "get_current_preset",
  "device.currentPreset": "get_current_preset",
  "device.identity": "get_qc_identity"
};

export interface AppMcpOptions {
  accessMode: ControlAccessMode;
  callTool(name: string, args: Record<string, unknown>): Promise<any>;
  confirm(message: string): Promise<boolean>;
  onStatus?(status: any): void;
  /** Captured with the accepted UI observation, never a later guard read. */
  expectedBrokerTarget?: BrokerTarget;
  brokerTargetGuardSupported?: boolean;
}

/** Decode the actual MCP tool envelope before gateway result validation. */
export function parseAppMcpResult(result: any): any {
  if (result?.isError) throw new Error(result.content?.map((item: any) => item.text ?? "").join("\n") || "QC Remote request failed.");
  const text = result?.content?.find((item: any) => item.type === "text")?.text;
  if (!text) return result;
  try { return JSON.parse(text); } catch { return text; }
}

/** The MCP action schemas spell every operand in snake_case. */
export function snakeCaseKey(key: string): string {
  return key.replace(/[A-Z]/g, letter => `_${letter.toLowerCase()}`);
}

export function appMcpRequest(method: string, args: Record<string, unknown> = {}) {
  if (!GATEWAY_RPCS.has(method)) throw new Error(`Unknown app gateway method: ${method}`);
  const readTool = APP_MCP_READ_TOOLS[method];
  if (readTool) {
    if (Object.keys(args).length) throw new Error(`${method} does not accept arguments.`);
    return { name: readTool, arguments: {} };
  }
  const action = ACTION_BY_RPC.get(method);
  if (!action) throw new Error(`The cloud host does not publish the ${method} action.`);
  const params = Object.fromEntries(Object.entries(args).map(([key, value]) => [snakeCaseKey(key), value]));
  if (action.name === "list_models") params.query = null;
  if (action.name === "rename_current_preset") {
    params.new_name = params.name;
    delete params.name;
    // Native UI confirmation is not part of the MCP action schema. The
    // persistent-write confirmation below is still required on this route.
    delete params.confirm_rename;
  }
  return { name: "execute_qc_action", arguments: { action: action.name, params } };
}

export async function invokeAppMcp(method: string, args: Record<string, unknown>, options: AppMcpOptions): Promise<any> {
  const input = { ...args };
  const expectedBrokerTarget = options.expectedBrokerTarget ? { ...options.expectedBrokerTarget } : undefined;
  brokerTargetKey(expectedBrokerTarget);
  const guardSupported = options.brokerTargetGuardSupported === true;
  const action = ACTION_BY_RPC.get(method);
  if (action) {
    if (!controlAccessPermitsAction(options.accessMode, action.name)) throw new Error(`The current ${options.accessMode} access mode does not allow ${action.name}.`);
    if (action.classification !== "read" && expectedBrokerTarget && !guardSupported) throw new Error("The cloud service cannot safely bind this write to the selected device. Update QC Remote Cloud before editing.");
    if (["risky-write", "persistent-write"].includes(action.classification) || ["show_tuner", "show_gig_view", "copy_scene"].includes(action.name)) {
      if (!await options.confirm(`${action.description}\n\nContinue with ${action.name.replaceAll("_", " ")}?`)) throw new Error("Cancelled.");
      const required: readonly string[] = action.required;
      if (required.includes("confirm_risky_operation")) input.confirmRiskyOperation = true;
      if (required.includes("confirm_persistent_write")) input.confirmPersistentWrite = true;
    }
  }
  const request = appMcpRequest(method, input);
  const requestArgs = action && expectedBrokerTarget && guardSupported
    ? { ...request.arguments, expectedBrokerTarget } : request.arguments;
  let result;
  try { result = await options.callTool(request.name, requestArgs); }
  catch (error) {
    if (method === "device.identity" && /not found|not allowed|unknown tool|METHOD_NOT_ALLOWED/i.test(String(error))) throw new Error("Device information requires an updated QC Remote cloud service and paired host. Existing sign-in is preserved.");
    throw error;
  }
  if (method === "system.status") options.onStatus?.(result);
  if (["device.reconnect", "device.resetSession", "device.disconnect"].includes(method)) {
    if (result === null || typeof result !== "object") throw new Error("The paired host returned a malformed connection response. Refresh before editing.");
    const source = result?.source;
    if (source !== undefined && source !== "device" && source !== "virtual") throw new Error("The paired host returned an unsupported connection source. Refresh before editing.");
    const restarted = result?.restarted ?? result?.changed !== false;
    let runtime;
    // The action already took effect on the host. A failed status read must not report it as
    // failed; it returns a fail-closed state with no target, so writes stay fenced.
    try { runtime = await options.callTool("get_qc_status", {}); } catch { runtime = undefined; }
    if (!runtime) return { phase: "needs-attention", detail: "The paired host accepted the request, but its status could not be read. Refresh to continue.", demo: false, source: "device", brokerTarget: undefined, restarted };
    const target = selectedBrokerTarget(runtime);
    if (source === "virtual" && !target && runtime.targetReady !== false) throw new Error("The paired host demo has no broker target identity. Update the host before editing.");
    options.onStatus?.(runtime);
    return { phase: runtime.targetReady === false ? "disconnected" : target ? "ready" : runtime.usbDiagnostics?.phase === "ready" ? "ready" : runtime.usbDiagnostics?.phase === "syncing" ? "syncing" : runtime.usbDiagnostics?.connected ? "opening" : "disconnected", detail: target?.source === "demo" ? "The paired host’s demo device is ready." : runtime.usbDiagnostics?.detail ?? "Remote host connection updated.", demo: false, source: "device", brokerTarget: target, restarted };
  }
  return result;
}
