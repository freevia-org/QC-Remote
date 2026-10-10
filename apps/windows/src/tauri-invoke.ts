async function callTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!window.__TAURI_INTERNALS__) {
    throw new Error("Desktop runtime is not active. Start with npm run tauri:dev.");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(command, args);
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export { callTauri };
