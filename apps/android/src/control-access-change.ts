import type { ControlAccessMode } from "@qc-remote/core";

/** Serialize access changes immediately, before React can disable the selector. */
export function createControlAccessChange(options: {
  write(mode: ControlAccessMode): Promise<{ accessMode: ControlAccessMode }>;
  read(): Promise<{ accessMode: ControlAccessMode }>;
  observed(mode: ControlAccessMode): void;
  uncertain(): void;
  pending(value: boolean): void;
  failed(error: unknown): void;
}) {
  let running = false;
  return async (mode: ControlAccessMode): Promise<void> => {
    if (running) return;
    running = true;
    options.pending(true);
    try {
      const status = await options.write(mode);
      options.observed(status.accessMode);
    } catch (error) {
      options.uncertain();
      try { options.observed((await options.read()).accessMode); } catch { /* Keep uncertainty visible until an authoritative status arrives. */ }
      options.failed(error);
    } finally { running = false; options.pending(false); }
  };
}
