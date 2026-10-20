import { GATEWAY_CLIENT_METHODS } from "@qc-remote/client";
import { controlAccessPermitsAction, type ControlAccessMode } from "./control-access.ts";
import { SHARED_QC_ACTIONS } from "./generated-actions.ts";

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
}

/** Decode the actual MCP tool envelope before gateway result validation. */
export function parseAppMcpResult(result: any): any {
  if (result?.isError) throw new Error(result.content?.map((item: any) => item.text ?? "").join("\n") || "QC Remote request failed.");
  const text = result?.content?.find((item: any) => item.type === "text")?.text;
  if (!text) return result;
  try { return JSON.parse(text); } catch { return text; }
}

export function appMcpRequest(method: string, args: Record<string, unknown> = {}) {
  if (!Object.values(GATEWAY_CLIENT_METHODS).some(entry => entry.rpc === method)) throw new Error(`Unknown app gateway method: ${method}`);
  const readTool = APP_MCP_READ_TOOLS[method];
  if (readTool) {
    if (Object.keys(args).length) throw new Error(`${method} does not accept arguments.`);
    return { name: readTool, arguments: {} };
  }
  const action = SHARED_QC_ACTIONS.find(entry => entry.rpc === method);
  if (!action) throw new Error(`The cloud host does not publish the ${method} action.`);
  const params = Object.fromEntries(Object.entries(args).map(([key, value]) => [key.replace(/[A-Z]/g, letter => `_${letter.toLowerCase()}`), value]));
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
  const action = SHARED_QC_ACTIONS.find(entry => entry.rpc === method);
  if (action) {
    if (!controlAccessPermitsAction(options.accessMode, action.name)) throw new Error(`The current ${options.accessMode} access mode does not allow ${action.name}.`);
    if (["risky-write", "persistent-write"].includes(action.classification) || ["show_tuner", "show_gig_view", "copy_scene"].includes(action.name)) {
      if (!await options.confirm(`${action.description}\n\nContinue with ${action.name.replaceAll("_", " ")}?`)) throw new Error("Cancelled.");
      const required: readonly string[] = action.required;
      if (required.includes("confirm_risky_operation")) input.confirmRiskyOperation = true;
      if (required.includes("confirm_persistent_write")) input.confirmPersistentWrite = true;
    }
  }
  const request = appMcpRequest(method, input);
  let result;
  try { result = await options.callTool(request.name, request.arguments); }
  catch (error) {
    if (method === "device.identity" && /not found|not allowed|unknown tool|METHOD_NOT_ALLOWED/i.test(String(error))) throw new Error("Device information requires an updated QC Remote cloud service and paired host. Existing sign-in is preserved.");
    throw error;
  }
  if (method === "system.status") options.onStatus?.(result);
  if (["device.reconnect", "device.resetSession", "device.disconnect"].includes(method)) {
    const runtime = await options.callTool("get_qc_status", {});
    options.onStatus?.(runtime);
    return { phase: runtime.usbDiagnostics?.phase === "ready" ? "ready" : runtime.usbDiagnostics?.phase === "syncing" ? "syncing" : runtime.usbDiagnostics?.connected ? "opening" : "disconnected", detail: runtime.usbDiagnostics?.detail ?? "Remote host connection updated.", demo: false, source: "device", restarted: result?.restarted ?? result?.changed !== false };
  }
  return result;
}
