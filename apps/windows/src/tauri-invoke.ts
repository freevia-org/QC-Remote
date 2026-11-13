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

/**
 * Bound a native read. `callTauri` has no timeout of its own, so one command
 * that never answers would otherwise hold whatever awaits it for the life of
 * the window. The late result, if it ever comes, is discarded.
 */
function withTimeout<T>(operation: Promise<T>, ms: number, what = "The request"): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} did not answer within ${Math.round(ms / 1000)} s.`)), ms);
  });
  return Promise.race([operation, timeout]).finally(() => { if (timer !== undefined) clearTimeout(timer); });
}

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export { callTauri, withTimeout };
