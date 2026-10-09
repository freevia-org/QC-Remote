import { useMemo, useState, type ReactNode } from "react";
import { QcConfirm, QcTextPrompt, type WorkflowPrompts } from "@qc-remote/ui";

/**
 * Android's own replacement for window.prompt and window.confirm, ported from
 * apps/windows/src/use-windows-prompts.tsx (same rationale, same shape) -
 * kept in its own per-platform file for the same reason that one is: App.tsx
 * itself is never supposed to call window.confirm/window.prompt directly,
 * and this is the one legitimate place that does, because it IS the
 * abstraction everything else is meant to funnel through instead.
 *
 * Both are QC Remote's own chrome now. The text a prompt collects (a renamed
 * preset, eventually a backup's file name) is either a computer-side concept
 * the Quad Cortex has no notion of, or is only ever entered through a device
 * screen this does not attempt to reconstruct; and a confirmation, while it
 * raises no device-fidelity question, still has to be read - which is what
 * the OS dialog made hardest. Both components are shared qc-ui code carrying
 * their own styling, so the touch-friendly overlay works unmodified here.
 *
 * Nothing in the Android shell currently calls `confirm`: the workflow
 * confirmations are gone, and the relay unpair still asks through
 * window.confirm in App.tsx rather than through here. Routing that one call
 * through this adapter would style it too.
 */
export function useAndroidPrompts(): { prompts: WorkflowPrompts; overlay: ReactNode } {
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
