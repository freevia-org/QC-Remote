async function callTauri<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!window.__TAURI_INTERNALS__) {
    throw new Error("Desktop runtime is not active. Start with npm run tauri:dev.");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  try {
    return await invoke<T>(command, args);
  } catch (failure) {
    // Tauri serializes native errors as objects, not JavaScript Error instances.
    if (failure instanceof Error) throw failure;
    const message = typeof failure === "object" && failure !== null && "message" in failure
      ? String(failure.message) : String(failure);
    throw Object.assign(new Error(message), typeof failure === "object" && failure !== null ? failure : {});
  }
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export { callTauri };
