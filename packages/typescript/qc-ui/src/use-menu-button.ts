import { useCallback, useEffect, useId, useRef, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { focusUserOpenedTarget } from "./user-open-focus";

/**
 * One keyboard model for every host-side popover menu opened by a button,
 * following the WAI-ARIA APG menu-button and menu patterns
 * (https://www.w3.org/WAI/ARIA/apg/patterns/menu-button/,
 * https://www.w3.org/WAI/ARIA/apg/patterns/menubar/):
 *
 * - opening moves focus to the first item (ArrowUp on the button: the last);
 * - ArrowDown/ArrowUp move between items (wrapping), Home/End to the ends;
 * - Escape closes and returns focus to the button;
 * - Tab/Shift+Tab close and continue the tab sequence from the button;
 * - an item is reached by arrows only (tabIndex -1); an unavailable item is
 *   aria-disabled, stays focusable, and activating it does nothing;
 * - activating an available item closes the menu and returns focus to the
 *   button, unless the item opens another surface, which then owns focus.
 *
 * The open state stays with the caller. Every way of opening - click, Enter,
 * Space, ArrowDown, ArrowUp - runs the caller's `onOpen` first, so menus that
 * exclude one another stay exclusive whichever key opened them.
 *
 * The only scheduled work is the one focus move after opening. It lives in a
 * single handle that is cancelled whenever focus is moved immediately (an
 * arrow, Home/End, a second open while open), when the menu closes, and when
 * the component unmounts, so a stale frame can never overwrite a later move.
 */
export function useMenuButton({ open, setOpen, onOpen }: { open: boolean; setOpen: (open: boolean) => void; onOpen?: () => void }) {
  const menuId = useId();
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const focusRequest = useRef<"first" | "last" | null>(null);
  const pendingFrame = useRef<number | undefined>(undefined);
  const cancelPending = () => {
    if (pendingFrame.current !== undefined) cancelAnimationFrame(pendingFrame.current);
    pendingFrame.current = undefined;
  };

  // The menu is found by its ref, or by its id where a caller keeps its own ref on the element.
  const items = () => [...((menuRef.current ?? document.getElementById(menuId))?.querySelectorAll<HTMLElement>("[role='menuitem']") ?? [])];

  useEffect(() => {
    if (!open || !focusRequest.current) return;
    const request = focusRequest.current;
    cancelPending();
    pendingFrame.current = requestAnimationFrame(() => {
      pendingFrame.current = undefined;
      focusRequest.current = null;
      const list = items();
      (request === "first" ? list[0] : list[list.length - 1])?.focus();
    });
    return cancelPending;
  }, [open]);

  useEffect(() => cancelPending, []);

  /** Focus an item now, cancelling any scheduled focus move first. */
  const focusItem = (target: "first" | "last" | number) => {
    cancelPending();
    focusRequest.current = null;
    const list = items();
    const index = target === "first" ? 0 : target === "last" ? list.length - 1 : (target + list.length) % list.length;
    list[index]?.focus();
  };

  const openMenu = useCallback((focus: "first" | "last") => {
    onOpen?.();
    if (open) {
      // Already open: the effect will not run again, so move focus now.
      focusItem(focus);
      return;
    }
    focusRequest.current = focus;
    setOpen(true);
  }, [onOpen, open, setOpen]);

  /** Close; `restoreFocus` returns focus to the button synchronously. Any scheduled focus move is cancelled. */
  const close = useCallback((restoreFocus: boolean) => {
    cancelPending();
    focusRequest.current = null;
    setOpen(false);
    if (restoreFocus) triggerRef.current?.focus();
  }, [setOpen]);

  /** The button's click: open with focus on the first item, or close. Enter and Space reach here through the native click only. */
  const toggle = useCallback(() => {
    if (open) close(false);
    else openMenu("first");
  }, [close, open, openMenu]);

  const onTriggerKeyDown = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    // Enter and Space are left to the button's native click, so a key press
    // never toggles twice.
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      openMenu(event.key === "ArrowDown" ? "first" : "last");
    } else if (open && (event.key === "Home" || event.key === "End")) {
      event.preventDefault();
      event.stopPropagation();
      focusItem(event.key === "Home" ? "first" : "last");
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    }
  };

  const onMenuKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const list = items();
    const index = list.indexOf(document.activeElement as HTMLElement);
    const moveTo = (target: number) => {
      event.preventDefault();
      event.stopPropagation();
      focusItem(target);
    };
    if (event.key === "ArrowDown") moveTo(index < 0 ? 0 : index + 1);
    else if (event.key === "ArrowUp") moveTo(index < 0 ? list.length - 1 : index - 1);
    else if (event.key === "Home") moveTo(0);
    else if (event.key === "End") moveTo(list.length - 1);
    else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    } else if (event.key === "Tab") {
      // No preventDefault: focus moves to the button first, so the browser's
      // own Tab continues from there to the next or previous element.
      close(false);
      triggerRef.current?.focus();
    }
  };

  /**
   * Props for one item. `run` executes the item's action; `opensSurface` items
   * leave focus to the surface they open. With `focusTarget`, the item itself then
   * focuses that destination's section (user-open-focus.ts); a screen that
   * opens any other way never takes focus. An unavailable item keeps focus and
   * the menu open, and never runs.
   */
  const itemProps = (run: () => void, { unavailable = false, opensSurface = false, focusTarget }: { unavailable?: boolean; opensSurface?: boolean; focusTarget?: string } = {}) => ({
    role: "menuitem" as const,
    tabIndex: -1,
    "aria-disabled": unavailable || undefined,
    onClick: (event: ReactMouseEvent<HTMLElement>) => {
      if (unavailable) {
        event.preventDefault();
        return;
      }
      close(!opensSurface);
      run();
      if (opensSurface && focusTarget) focusUserOpenedTarget(focusTarget);
    }
  });

  return {
    menuId,
    triggerRef,
    menuRef,
    open,
    toggle,
    close,
    openMenu,
    triggerProps: {
      ref: triggerRef,
      "aria-haspopup": "menu" as const,
      "aria-expanded": open,
      "aria-controls": open ? menuId : undefined,
      onKeyDown: onTriggerKeyDown
    },
    menuProps: { id: menuId, ref: menuRef, role: "menu" as const, onKeyDown: onMenuKeyDown },
    itemProps
  };
}
