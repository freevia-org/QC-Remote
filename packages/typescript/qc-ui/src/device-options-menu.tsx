import { useState } from "react";
import { QcEditorIcon } from "./theme-icons";
import type { ParameterContextMenuItem, ParameterEditorContextAction } from "./parameter-menu";
import "./device-options-menu.css";

/*
 * The block editor's options menu (the unit's zenUI::_OptionsPopup over its ParameterEditor).
 *
 * On the unit this is a screen-level layer, not a dropdown of the editor: a 320 x 420 panel at 32,30 on the 800x480
 * frame, seven 60px rows visible and the rest scrolled, over a screen it has dimmed (the Grid behind it and the editor
 * body alike). It is therefore rendered as a sibling of the editor, directly under the screen canvas, so its box is the
 * screen's and not the editor's. The scrim is one full-bleed button: a tap anywhere outside the panel closes the menu,
 * which is what the unit does and what the app's mirror route relies on (the unit's menu is closed with a tap on an empty
 * area outside the popup, and the app only ever opens or closes it, never touches a row).
 */

const ROW = 60;
const TRACK_INSET_TOP = 12;
const TRACK_LENGTH = 395;
const PANEL_HEIGHT = 420;

export function QcDeviceOptionsMenu({ items, enabled, coversScreen, onDismiss, onRun }: {
  items: readonly ParameterContextMenuItem[];
  enabled?: Partial<Record<ParameterEditorContextAction, boolean>>;
  /** A full-screen editor has no Grid above it, so the editor's dim covers the whole screen. */
  coversScreen: boolean;
  onDismiss: () => void;
  onRun: (action: ParameterEditorContextAction) => void;
}) {
  const [scroll, setScroll] = useState(0);
  const content = items.length * ROW;
  const overflow = content > PANEL_HEIGHT;
  const thumb = Math.floor(TRACK_LENGTH * PANEL_HEIGHT / content);
  const range = content - PANEL_HEIGHT;
  const thumbTop = TRACK_INSET_TOP + (range > 0 ? Math.round((TRACK_LENGTH - thumb) * Math.min(1, scroll / range)) : 0);
  return <>
    <button type="button" className={`qc-device-options-scrim${coversScreen ? " is-covering" : ""}`} data-qc-options-scrim="" aria-label="Close device contextual menu" onClick={onDismiss} />
    <div className="qc-device-options-panel" role="menu" aria-label="Device contextual menu">
      <div className="qc-device-options-list" onScroll={(event) => setScroll(event.currentTarget.scrollTop)}>
        {items.map((item) => {
          const unavailable = item.disabled || enabled?.[item.action] === false;
          return <button key={item.action} type="button" role="menuitem" className={`${item.disabled ? "is-disabled" : ""}`} disabled={unavailable} onClick={() => onRun(item.action)}><span><QcEditorIcon kind={item.icon} /></span>{item.label}</button>;
        })}
      </div>
      {overflow && <i className="qc-device-options-thumb" aria-hidden="true" style={{ top: `${thumbTop}px`, height: `${thumb}px` }} />}
    </div>
  </>;
}
