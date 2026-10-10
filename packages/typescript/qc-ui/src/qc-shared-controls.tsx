import { useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type ReactNode, type RefObject } from "react";
import { QcEditorIcon, QcUiIcon } from "./theme-icons";
import "@qc-remote/theme/app-dialog.css";

/** The 24px scene-letter tile used by every CorOS header control. */
export function QcSceneBadge({ letter, className, tone = "active" }: { letter: string; className?: string; tone?: "active" | "inactive" }) {
  return <span className={`qc-scene-letter-badge is-${tone}${className ? ` ${className}` : ""}`}>{letter}</span>;
}

/** One scene shuttle; screens only own its placement, never its geometry. */
export function QcSceneSelector({ letter, className, onPrevious, onNext }: { letter: string; className?: string; onPrevious?: () => void; onNext?: () => void }) {
  return <span className={`qc-scene-selector${className ? ` ${className}` : ""}`} aria-label="Scene selector">
    <button type="button" aria-label="Previous scene" onClick={onPrevious}><QcEditorIcon kind="scene-previous" /></button>
    <span className="qc-scene-selector-badge-slot"><QcSceneBadge letter={letter} /></span>
    <button type="button" aria-label="Next scene" onClick={onNext}><QcEditorIcon kind="scene-next" /></button>
  </span>;
}

/** The shared editor-header footswitch target, including its assignment letter; the unit draws "?" until one is assigned. */
export function QcEditorFootswitchAssignment({ letter, className, pending = false, ...buttonProps }: { letter?: string; className?: string; pending?: boolean } & ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button {...buttonProps} className={`editor-stomp${pending ? " is-assigning" : ""}${className ? ` ${className}` : ""}`}><QcEditorIcon kind="footswitch" /><span>{pending ? "?" : letter ?? "?"}</span></button>;
}

/**
 * The unit's own "Press the target footswitch to assign" prompt
 * (a 420x294 dialog at 190,93 over a two-layer wash).
 * The actual assign/reassign behavior already runs on a key press or a
 * physical footswitch release; this only narrates it and offers Cancel and
 * Unassign, since the device shows both. Latching/Momentary selection is a
 * separate per-switch setting (`setStompMomentary`), not part of this prompt
 * yet - left out here rather than guessed at.
 */
export function QcFootswitchAssignmentPrompt({ assignedLetter, onCancel, onUnassign }: { assignedLetter?: string; onCancel(): void; onUnassign(): void }) {
  return <div className="qc-footswitch-assignment-wash" role="presentation">
    <div className="qc-footswitch-assignment-dialog" role="alertdialog" aria-label="Footswitch assignment">
      <h1>Assign footswitch</h1>
      <p>Press the target footswitch to assign</p>
      <footer>
        <button type="button" onClick={onCancel}>CANCEL</button>
        <button type="button" className="is-primary" disabled={!assignedLetter} onClick={onUnassign}>UNASSIGN</button>
      </footer>
    </div>
  </div>;
}

/**
 * Keyboard handling shared by the app's own modal dialogs: Escape answers no,
 * Tab stays inside the dialog, and Enter submits only when it is typed on the
 * dialog surface itself or into `enterTarget` (the prompt's text field).
 *
 * A key the dialog handles stops here: Escape and a submitting Enter call
 * stopPropagation as well as preventDefault, so a window-level shortcut
 * handler never acts on the same key press (Escape cancelling a confirmation
 * must not also close the surface the dialog was raised over).
 *
 * Enter on a focused button is deliberately left alone. A keydown on a button
 * bubbles up to the dialog, and handling it here - with `preventDefault()`
 * suppressing the button's own activation - is how a keyboard user who tabbed
 * to Cancel and pressed Enter used to confirm the overwrite. The focused
 * button's native activation already answers correctly, Cancel included.
 */
export function useDialogFocusTrap<T extends HTMLElement = HTMLDivElement>({ focusable, onCancel, onEnter, enterTarget }: { focusable: string; onCancel(): void; onEnter(): void; enterTarget?: RefObject<HTMLElement | null> }) {
  const dialogRef = useRef<T>(null);
  const onKeyDown = (event: ReactKeyboardEvent<T>) => {
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onCancel(); return; }
    if (event.key === "Enter") {
      if (event.target !== event.currentTarget && (!enterTarget?.current || event.target !== enterTarget.current)) return;
      event.preventDefault(); event.stopPropagation(); onEnter(); return;
    }
    if (event.key !== "Tab") return;
    const elements = [...(dialogRef.current?.querySelectorAll<HTMLElement>(focusable) ?? [])];
    // Nothing to move to (every control disabled): Tab must still not leave.
    if (elements.length === 0) { event.preventDefault(); dialogRef.current?.focus(); return; }
    const edge = elements[event.shiftKey ? 0 : elements.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    elements[event.shiftKey ? elements.length - 1 : 0].focus();
  };
  return { dialogRef, onKeyDown };
}

/**
 * A generic, app-native replacement for `window.prompt`. There is no device
 * screen to match here - the text this collects (a backup's file name, a
 * renamed preset) is either a computer-side concept the Quad Cortex itself
 * has no notion of, or is only ever entered through a device screen this
 * component does not attempt to reconstruct - so it is styled like the rest
 * of QC Remote's own chrome (matching `local-unpair-prompt`'s tokens), not
 * like CorOS. Its rules are in `@qc-remote/theme/app-dialog.css`, imported
 * above; they used to live in `apps/windows/src/styles.css`, where Android -
 * which renders this same component for the preset rename - never saw them.
 */
export function QcTextPrompt({ title, message, initialValue, submitLabel = "Save", onSubmit, onCancel }: { title: string; message?: string; initialValue: string; submitLabel?: string; onSubmit(value: string): void; onCancel(): void }) {
  const [value, setValue] = useState(initialValue);
  const inputRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const frame = requestAnimationFrame(() => { inputRef.current?.focus(); inputRef.current?.select(); });
    return () => cancelAnimationFrame(frame);
  }, []);
  const trimmed = value.trim();
  const submit = () => { if (trimmed) onSubmit(trimmed); };
  const { dialogRef, onKeyDown } = useDialogFocusTrap({ focusable: "input, button:not([disabled])", onCancel, onEnter: submit, enterTarget: inputRef });
  return <div className="qc-text-prompt-wash" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onCancel(); }}>
    <div className="qc-text-prompt-dialog" role="alertdialog" aria-modal="true" aria-label={title} ref={dialogRef} onKeyDown={onKeyDown}>
      <strong>{title}</strong>
      {message && <p>{message}</p>}
      <input ref={inputRef} type="text" value={value} onChange={(event) => setValue(event.target.value)} />
      <footer>
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="button" className="primary" disabled={!trimmed} onClick={submit}>{submitLabel}</button>
      </footer>
    </div>
  </div>;
}

/** A confirmation waiting for an answer, in QC Remote's own chrome.
 *
 * `window.confirm` renders as the OS's dialog, which under Tauri announces
 * itself as "tauri.localhost says..." - a host the user never chose to talk
 * to, above a sentence squeezed into a system font. Every confirmation left
 * in QC Remote describes something about to be overwritten, so that sentence
 * is the entire reason the dialog exists and is worth being able to read.
 *
 * Escape, and a click on the wash outside the dialog, both answer no: a
 * dismissal is never taken as consent. The default label stays "OK" so that
 * replacing the OS dialog changes how it looks and nothing about what it
 * says. Styling lives in `@qc-remote/theme/app-dialog.css`, imported above, on
 * shared palette tokens, so it renders the same in the Android webview. Enter
 * on a focused button activates that button, never the confirmation.
 */
export function QcConfirm({ message, confirmLabel = "OK", onConfirm, onCancel }: { message: string; confirmLabel?: string; onConfirm(): void; onCancel(): void }) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  const messageId = useId();
  useEffect(() => {
    const frame = requestAnimationFrame(() => confirmRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);
  const { dialogRef, onKeyDown } = useDialogFocusTrap({ focusable: "button:not([disabled])", onCancel, onEnter: onConfirm });
  return <div className="qc-confirm-wash" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onCancel(); }}>
    <div className="qc-confirm-dialog" role="alertdialog" aria-modal="true" aria-label="Confirmation" aria-describedby={messageId} ref={dialogRef} onKeyDown={onKeyDown}>
      <p id={messageId}>{message}</p>
      <footer>
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="button" className="primary" ref={confirmRef} onClick={onConfirm}>{confirmLabel}</button>
      </footer>
    </div>
  </div>;
}

/** A Grid add-device tile; screens supply only placement through className. */
export function QcGridAddTile({ className }: { className?: string }) {
  return <i className={className}><QcUiIcon kind="add" /></i>;
}

/** State-aware action cell used by the Directory multi-select toolbar. */
export function QcMultiSelectAction({ enabled, className, children }: { enabled: boolean; className?: string; children: ReactNode }) {
  return <button type="button" className={`qc-multiselect-action ${enabled ? "is-enabled" : "is-disabled"}${className ? ` ${className}` : ""}`} aria-disabled={!enabled}>{children}</button>;
}

/**
 * The movable microphone marker shared by reference Cab fixtures and the live
 * editor. Its parent owns pointer handling; x/y remain real parameters.
 */
export function QcCabMicrophoneTarget({ x, y, direction = "right" }: { x: number; y: number; direction?: "left" | "right" }) {
  return <span
    className={`qc-cab-microphone-target is-${direction}`}
    style={{ "--qc-cab-mic-x": `${x}%`, "--qc-cab-mic-y": `${y}%` } as CSSProperties}
    aria-hidden="true"
  ><i /><i /><i /></span>;
}

export function QcFootswitchAssignmentMode({ mode, selected = false }: { mode: "latching" | "momentary"; selected?: boolean }) {
  const icon = mode === "latching" ? "footswitch" : "momentary";
  const label = mode === "latching" ? "Latching" : "Momentary";
  return <button type="button" className={selected ? "is-active" : undefined} aria-pressed={selected}><QcEditorIcon kind={icon} />{label}</button>;
}

export function QcPillToggleLabels({ labels, index, className }: { labels: readonly ReactNode[]; index: number; className?: string }) {
  return <span className={`qc-pill-toggle-labels${className ? ` ${className}` : ""}`}>
    {labels.map((label, labelIndex) => <b key={labelIndex} className={labelIndex === index ? "is-active" : undefined}>{label}</b>)}
  </span>;
}
