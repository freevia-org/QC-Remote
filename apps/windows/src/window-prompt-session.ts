export type WindowPromptRequest =
  | { id: number; kind: "confirm"; message: string; resolve(value: boolean): void }
  | { id: number; kind: "text"; title: string; initialValue: string; resolve(value: string | null): void };

/** Replacing a dialog is a cancellation, never an abandoned promise. */
export function createWindowPromptSession() {
  let active: WindowPromptRequest | undefined;
  let nextId = 0;
  let publish: ((request: WindowPromptRequest | undefined) => void) | undefined;
  let disposed = false;
  const cancel = () => {
    const previous = active;
    active = undefined;
    if (previous?.kind === "confirm") previous.resolve(false);
    else previous?.resolve(null);
  };
  const replace = (request: WindowPromptRequest) => {
    cancel();
    if (disposed) {
      if (request.kind === "confirm") request.resolve(false);
      else request.resolve(null);
      return;
    }
    active = request;
    publish?.(active);
  };
  return {
    attach(listener: (request: WindowPromptRequest | undefined) => void) {
      disposed = false;
      publish = listener;
      listener(active);
      return () => { disposed = true; publish = undefined; cancel(); };
    },
    confirm(message: string) {
      return new Promise<boolean>((resolve) => replace({ id: ++nextId, kind: "confirm", message, resolve }));
    },
    prompt(title: string, initialValue = "") {
      return new Promise<string | null>((resolve) => replace({ id: ++nextId, kind: "text", title, initialValue, resolve }));
    },
    answer(id: number, value: boolean | string | null) {
      if (!active || active.id !== id) return;
      const request = active;
      active = undefined;
      publish?.(undefined);
      if (request.kind === "confirm") request.resolve(value === true);
      else request.resolve(typeof value === "string" ? value : null);
    }
  };
}
