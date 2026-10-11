import { QC_COLORS } from "@qc-remote/theme";
import type { CorOsParameterEditorProps } from "./parameter-editor";
import { parameterEditorAccent, parameterEditorControlSlots, parameterEditorPageSize } from "./parameter-model.ts";

export interface QcParameterEditorLed { active: boolean; assigned: boolean; color: string }

/**
 * The ten encoder-row LEDs while a parameter editor is open: lit where the
 * page has a control, in the block's accent (pitch blue for the cab's
 * microphone controls). The one derivation for the display and for any host
 * that draws the unit's switches itself.
 */
export function qcParameterEditorLeds(editor: Pick<CorOsParameterEditorProps, "details" | "accent" | "page"> | undefined): readonly QcParameterEditorLed[] | undefined {
  if (!editor) return undefined;
  const accent = parameterEditorAccent(editor.details.name, editor.accent);
  const size = parameterEditorPageSize(editor.details.category, editor.details.parameters);
  const visible = parameterEditorControlSlots(
    editor.details.parameters.filter((parameter) => parameter.normalizedValue !== null),
    editor.details.category,
    editor.page,
    size
  );
  const cab = /\bcab\b/i.test(editor.details.category);
  return Array.from({ length: 10 }, (_, index) => ({
    active: Boolean(visible[index]),
    assigned: Boolean(visible[index]),
    color: cab && ((editor.page === 0 && index >= 5 && index <= 8) || (editor.page === 1 && index === 2)) ? QC_COLORS.category.pitch : accent
  }));
}
