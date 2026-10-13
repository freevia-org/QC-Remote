import { useEffect, useMemo, useState, type ReactNode } from "react";
import { QcConfirm, QcTextPrompt, type WorkflowPrompts } from "@qc-remote/ui";
import { registerWorkflowConfirmation } from "./workflow-confirmations";

export function useAndroidPrompts(): { prompts: WorkflowPrompts; overlay: ReactNode } {
  const [textPrompt, setTextPrompt] = useState<{ title: string; initialValue: string; resolve: (value: string | null) => void }>();
  const [confirmation, setConfirmation] = useState<{ message: string; resolve: (value: boolean) => void }>();
  const confirm = (message: string) => new Promise<boolean>((resolve) => setConfirmation({ message, resolve }));
  useEffect(() => registerWorkflowConfirmation(confirm), []);
  const prompts = useMemo<WorkflowPrompts>(() => ({
    confirm,
    prompt: (message, initialValue) => new Promise<string | null>((resolve) => setTextPrompt({ title: message, initialValue, resolve }))
  }), []);
  const answer = (accepted: boolean) => { confirmation?.resolve(accepted); setConfirmation(undefined); };
  const overlay = <>
    {confirmation && <QcConfirm message={confirmation.message} onConfirm={() => answer(true)} onCancel={() => answer(false)} />}
    {textPrompt && <QcTextPrompt title={textPrompt.title} initialValue={textPrompt.initialValue} onSubmit={(value) => { textPrompt.resolve(value); setTextPrompt(undefined); }} onCancel={() => { textPrompt.resolve(null); setTextPrompt(undefined); }} />}
  </>;
  return { prompts, overlay };
}
