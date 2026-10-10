import type { KeyboardEvent } from "react";

/**
 * Focus for a screen the user opened from a menu, and only for that.
 *
 * A screen opened from a menu replaces the control that had focus, so without
 * help focus falls to the document body. The same screen can also open
 * because the app follows the Quad Cortex's own screen, and moving focus then
 * would take it from whatever the user is doing. So nothing happens when a
 * screen mounts: focus moves only from the user's own menu activation, and
 * only to the destination that activation named. The destination's labelled
 * section carries data-qc-open-target="<destination>" and tabIndex={-1}, so it
 * takes focus without joining the tab order.
 */

/** How many frames the opened destination may take to appear. */
const OPEN_TARGET_FRAMES = 2;

/**
 * Called by the menu item the user activated, after its action ran, or by a
 * dialog the user dismissed, to return to the button that opened it. Within
 * the next frames, focuses that destination's section if it is on screen and
 * focus is still on the body (the activated item was removed with its menu).
 * It never moves focus the user has put somewhere since, never focuses a
 * different destination, and never focuses inside an inert subtree. An inert
 * descendant, such as a read-only reference pane, does not block the section.
 */
export function focusUserOpenedTarget(destination: string, frames = OPEN_TARGET_FRAMES) {
  if (typeof window === "undefined") return;
  window.requestAnimationFrame(() => {
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const section = document.querySelector<HTMLElement>(`[data-qc-open-target="${destination}"]`);
    if (!section) {
      if (frames > 1) focusUserOpenedTarget(destination, frames - 1);
      return;
    }
    if (section.closest("[inert]")) return;
    section.focus({ preventScroll: true });
  });
}

/** Opener provenance belongs to committed destination state, never a pending frame. */
export function closeUserOpenedTarget(source: HTMLElement, close: () => void, returnFocusTarget?: "grid-menu") {
  const section = source.closest<HTMLElement>("[data-qc-open-target]");
  const ownedFocus = Boolean(section && section.contains(document.activeElement));
  close();
  if (returnFocusTarget && ownedFocus) focusUserOpenedTarget(returnFocusTarget);
}

export function escapeUserOpenedTarget(event: KeyboardEvent<HTMLElement>, close: () => void, returnFocusTarget?: "grid-menu", disabled = false) {
  if (event.key !== "Escape" || event.defaultPrevented) return;
  event.preventDefault();
  event.stopPropagation();
  if (!disabled) closeUserOpenedTarget(event.currentTarget, close, returnFocusTarget);
}
