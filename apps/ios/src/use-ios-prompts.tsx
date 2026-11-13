import { useEffect, type ReactNode } from "react";
import type { WorkflowPrompts } from "@qc-remote/ui";
// Imported by path, not by the "./use-android-prompts" specifier the build aliases to this file.
import { useAndroidPrompts as useSharedPrompts } from "@qc-remote/android-src/use-android-prompts.tsx";
import { registerWorkflowConfirmation } from "./workflow-confirmations";

/** The shared hook owns one promise per dialog (a replacement or unmount settles the old
 * one); iOS only routes its MCP confirmations through the same owner. */
export function useAndroidPrompts(): { prompts: WorkflowPrompts; overlay: ReactNode } {
  const shared = useSharedPrompts();
  const { confirm } = shared.prompts;
  useEffect(() => registerWorkflowConfirmation((message) => Promise.resolve(confirm(message))), [confirm]);
  return shared;
}
