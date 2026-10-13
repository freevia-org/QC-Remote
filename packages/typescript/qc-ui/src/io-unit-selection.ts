/**
 * What the unit's I/O Settings screen (zenUI::CheatSheetDialog) shows of its ports, read from its graphics tree: which
 * port or pair is selected, and which jacks have something plugged in. The unit's screen is the authority: the app's
 * I/O screen follows it on every open and while open.
 *
 * The tree carries no text for the heading ("Output 1/2" is drawn without a text node), so the selection is read from
 * the jack indicators. Each port is an Indicator (`image: 'icons/cheatsheet/<family>/<state>.png'`) followed by its
 * label (`text : 'OUT 1/L'`, or the headphones glyph). The selected port's indicator is `active`; the others are
 * `free` (nothing plugged) or `taken` (plugged). Observed on 4.1.0: OUT 1/L and OUT 2/R `xlr/active` with the
 * heading reading Output 1/2, and the headphones `jack/taken` and USB `usb/taken` drawn as the white plugged jack.
 */

export type IoView = "overview" | "input" | "output" | "send-return" | "return" | "expression" | "midi" | "capture" | "usb" | "headphones";

export interface IoUnitSelection {
  /** The selected port's view; undefined while the tree shows no active port (a frame mid-transition). */
  view?: IoView;
  /** The input jack, for the Input view. */
  inputId?: number;
  /** The sockets (the screen's own ids) that hold a plug. */
  plugged: readonly string[];
}

/** The unit's label for each port: its socket, and the view it selects. `overview` is the app's name for Output 3/4. */
const PORT_LABELS: Readonly<Record<string, { socket: string; view: IoView; inputId?: number }>> = {
  "INPUT 1": { socket: "input1", view: "input", inputId: 1 }, "INPUT 2": { socket: "input2", view: "input", inputId: 2 },
  "SEND 1": { socket: "send1", view: "send-return" }, "SEND 2": { socket: "send2", view: "send-return" },
  "RET 1": { socket: "ret1", view: "return" }, "RET 2": { socket: "ret2", view: "return" },
  "OUT 1/L": { socket: "out1", view: "output" }, "OUT 2/R": { socket: "out2", view: "output" },
  "OUT 3/L": { socket: "out3", view: "overview" }, "OUT 4/R": { socket: "out4", view: "overview" },
  "CAPTURE OUT": { socket: "capture-out", view: "capture" },
  "MIDI IN": { socket: "midi-in", view: "midi" }, "MIDI OUT": { socket: "midi-out", view: "midi" },
  "EXP 1": { socket: "exp1", view: "expression" }, "EXP 2": { socket: "exp2", view: "expression" },
  "USB": { socket: "usb", view: "usb" }
};
const HEADPHONES: { socket: string; view: IoView; inputId?: number } = { socket: "phones", view: "headphones" };

/** What the tree shows, or undefined when it is not the I/O screen. */
export function ioSelectionFromTree(tree: string): IoUnitSelection | undefined {
  if (!/^\s*zenUI::CheatSheetDialog\s*$/m.test(tree)) return undefined;
  let indicator: string | undefined;
  let selected: { view: IoView; inputId?: number } | undefined;
  const plugged: string[] = [];
  for (const line of tree.split(/\r?\n/)) {
    const image = line.match(/^\s*image\s*:\s*'icons\/cheatsheet\/[A-Za-z0-9_]+\/([A-Za-z0-9_]+)\.png'\s*$/);
    if (image) { indicator = image[1]; continue; }
    const label = line.match(/^\s*text\s*:\s*'([^']*)'\s*$/)?.[1];
    const port = label !== undefined && Object.hasOwn(PORT_LABELS, label) ? PORT_LABELS[label]
      : /^\s*image\s*:\s*'icons\/secondLevelNavigation\/headphones\.png'\s*$/.test(line) ? HEADPHONES : undefined;
    if (!port) continue;
    if (indicator === "taken") plugged.push(port.socket);
    // The first active port names the view: both jacks of a selected pair are active and name the same one.
    if (indicator === "active" && !selected) selected = { view: port.view, ...(port.inputId === undefined ? {} : { inputId: port.inputId }) };
    indicator = undefined;
  }
  return { ...selected, plugged };
}

/** A stable text for a selection, for comparing observations. */
export const ioSelectionKey = (selection: IoUnitSelection | undefined): string =>
  selection ? `${selection.view ?? ""}${selection.inputId ?? ""}|${[...selection.plugged].sort().join(",")}` : "";
