import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { QcConfirm, QcTextPrompt, type WorkflowPrompts } from "@qc-remote/ui";

type Dialog =
  | { kind: "confirm"; message: string; settle(value: boolean): void }
  | { kind: "text"; title: string; initialValue: string; settle(value: string | null): void };

/** One visible Android prompt owns one promise; replacement and teardown cancel it. */
export function useAndroidPrompts(): { prompts: WorkflowPrompts; overlay: ReactNode } {
  const [dialog, setDialog] = useState<Dialog>();
  const owned = useRef<Dialog | undefined>(undefined);
  const mounted = useRef(true);
  const cancel = (held: Dialog | undefined) => {
    if (!held) return;
    if (held.kind === "confirm") held.settle(false);
    else held.settle(null);
  };
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const previous = owned.current;
      owned.current = undefined;
      cancel(previous);
    };
  }, []);
  const prompts = useMemo<WorkflowPrompts>(() => {
    const install = (next: Dialog) => {
      if (!mounted.current) { cancel(next); return; }
      const previous = owned.current;
      owned.current = next;
      cancel(previous);
      setDialog(next);
    };
    return {
      confirm: (message) => new Promise<boolean>((settle) => install({ kind: "confirm", message, settle })),
      prompt: (title, initialValue) => new Promise<string | null>((settle) => install({ kind: "text", title, initialValue, settle }))
    };
  }, []);
  const answer = (held: Dialog, value: string | boolean | null) => {
    if (!mounted.current || owned.current !== held) return;
    owned.current = undefined;
    if (held.kind === "confirm") held.settle(value === true);
    else held.settle(typeof value === "string" ? value : null);
    setDialog(undefined);
  };
  const overlay = dialog?.kind === "confirm"
    ? <QcConfirm message={dialog.message} onConfirm={() => answer(dialog, true)} onCancel={() => answer(dialog, false)} />
    : dialog?.kind === "text"
      ? <QcTextPrompt title={dialog.title} initialValue={dialog.initialValue} onSubmit={(value) => answer(dialog, value)} onCancel={() => answer(dialog, null)} />
      : null;
  return { prompts, overlay };
}
