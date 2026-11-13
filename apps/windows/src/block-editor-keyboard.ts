/** Whether Escape should close the block editor rather than being consumed by
 * a nested interactive surface. The app-level handler runs in the bubble
 * phase, after a focused dialog or menu has had the chance to prevent it. */
export function shouldCloseBlockEditorOnEscape(
  event: Pick<KeyboardEvent, "key" | "defaultPrevented" | "target">,
  blockEditorOpen: boolean
): boolean {
  if (!blockEditorOpen || event.key !== "Escape" || event.defaultPrevented) return false;
  const target = event.target as HTMLElement | null;
  return !target?.closest('[aria-modal="true"], [role="menu"], [role="listbox"], [role="dialog"], [role="alertdialog"]');
}

/**
 * The Grid's single-key shortcuts (bank, tap tempo, bypass, delete, arrows) are
 * plain keys. With Ctrl, Meta or Alt held the keystroke is the host's own
 * (Ctrl+B, Ctrl+Shift+T, Alt+arrow), and the Grid must leave it alone.
 */
export function isPlainGridKey(event: { ctrlKey: boolean; metaKey: boolean; altKey: boolean }): boolean {
  return !event.ctrlKey && !event.metaKey && !event.altKey;
}
