import { useMemo, useState, type ReactNode } from "react";
import { QcConfirm, QcTextPrompt, type WorkflowPrompts } from "@qc-remote/ui";

/** Windows' replacement for `window.prompt` and `window.confirm`, kept in its
 * own file for the same reason browser-workflow-prompts.ts is: App.tsx itself
 * is never supposed to call window.confirm or window.prompt directly, and
 * this is the one legitimate place that would, because it IS the abstraction
 * everything else is meant to funnel through instead.
 *
 * Neither is the OS's dialog any more. `window.prompt` went first: the text it
 * collects (a backup's file name, a renamed preset) is either a computer-side
 * concept the Quad Cortex has no notion of, or is only ever entered through a
 * device screen this does not attempt to reconstruct, so replacing it with QC
 * Remote's own chrome was a UI-quality fix rather than a CorOS
 * reconstruction. `window.confirm` was left native on the reasoning that a
 * yes/no question raises no fidelity question - true of the question, but not
 * of the dialog: under Tauri it announces itself as "tauri.localhost says...",
 * naming a host the user never chose to talk to.
 *
 * What is left to confirm makes that worse rather than better. Every
 * confirmation in the app now describes a preset about to be overwritten and
 * what is on it, so the sentence is the whole point, and the OS dialog was the
 * one place it could not be read comfortably.
 */
export function useWindowsPrompts(): { prompts: WorkflowPrompts; overlay: ReactNode } {
  const [textPrompt, setTextPrompt] = useState<{ title: string; message?: string; initialValue: string; submitLabel?: string; resolve: (value: string | null) => void }>();
  const [confirmation, setConfirmation] = useState<{ message: string; resolve: (value: boolean) => void }>();
  const prompts = useMemo<WorkflowPrompts>(() => ({
    confirm: (message) => new Promise<boolean>((resolve) => setConfirmation({ message, resolve })),
    prompt: (message, initialValue) => new Promise<string | null>((resolve) => setTextPrompt({ title: message, initialValue, resolve }))
  }), []);
  const answer = (accepted: boolean) => { confirmation?.resolve(accepted); setConfirmation(undefined); };
  const overlay = <>
    {confirmation && <QcConfirm message={confirmation.message} onConfirm={() => answer(true)} onCancel={() => answer(false)} />}
    {textPrompt && <QcTextPrompt title={textPrompt.title} message={textPrompt.message} initialValue={textPrompt.initialValue} submitLabel={textPrompt.submitLabel} onSubmit={(value) => { textPrompt.resolve(value); setTextPrompt(undefined); }} onCancel={() => { textPrompt.resolve(null); setTextPrompt(undefined); }} />}
  </>;
  return { prompts, overlay };
}
