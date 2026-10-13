import type { GatewayTransport, GridBlock } from "@qc-remote/client";
import { classifyQcGraphicsTree, type QcObservedScreenKind } from "./use-adaptive-screen-polling.ts";

// Native 800 x 480 Grid cell centres. Browser pixels must never be forwarded:
// their scale depends on the window and form factor.
export const QC_GRID_SCREEN_COLUMNS = [101, 187, 272, 357, 443, 529, 615, 701] as const;
export const QC_GRID_SCREEN_ROWS = [147, 241, 335, 429] as const;
export const QC_PARAMETER_EDITOR_DONE = { x: 742, y: 30 } as const;

function requireGrid(tree: string): void {
  if (!/zenUI::Grid(?![A-Za-z0-9_])/.test(tree)) {
    throw new Error("The Quad Cortex is not showing the Grid. Return to the Grid, then select the block again.");
  }
}

function requireBareGrid(tree: string): void {
  requireGrid(tree);
  // The Grid remains in a menu/dialog tree. Its presence does not make a cell
  // coordinate safe to touch. Unknown popup/dialog/editor classes are refused
  // too; ordinary Grid children (labels, tiles and paths) remain admissible.
  const classes = [...tree.matchAll(/zenUI::([A-Za-z0-9_]+)/g)].map((match) => match[1]);
  if (classifyQcGraphicsTree(tree) !== "grid"
      || classes.some((name) => /(?:Popup|Dialog|Editor|Overlay|Panel)$/.test(name))) {
    throw new Error("The Quad Cortex Grid is covered by another screen. Close it, then select the block again.");
  }
}

/**
 * Mirror an app-side Grid selection onto the physical QC.
 *
 * No device-originated touchscreen/editor event appears in the complete raw
 * traffic captured for a physical Grid tap on retail CorOS 4.1.0. This
 * one-shot graphics-tree read is a safety preflight, not a watcher: it keeps a
 * stale app from replaying Grid coordinates into another QC screen.
 */
export async function openGridBlockOnDevice(
  gateway: GatewayTransport,
  block: GridBlock,
  stillCurrent: () => boolean = () => true
): Promise<boolean> {
  const { tree } = await gateway.graphicsTree();
  requireBareGrid(tree);
  if (!stillCurrent()) return false;
  const x = QC_GRID_SCREEN_COLUMNS[block.column];
  const y = QC_GRID_SCREEN_ROWS[block.row];
  if (x === undefined || y === undefined) throw new Error("The selected block is outside the Quad Cortex Grid.");
  await gateway.tapScreen(x, y);
  return true;
}

/**
 * Close the physical parameter editor only when CorOS says it is visible. When
 * the unit shows something else over its Grid (a splitter or lane editor, a
 * popup on the editor, a panel of a class the app does not know) no touch is
 * sent, because the check button is proven only on the parameter editor;
 * `onLeftOpen` then names what the unit still shows, so the app never closes
 * its editor and says nothing while the unit keeps its own open.
 */
export async function closeGridBlockOnDevice(
  gateway: GatewayTransport,
  stillCurrent: () => boolean = () => true,
  onLeftOpen?: (screen: QcObservedScreenKind) => void
): Promise<boolean> {
  const { tree } = await gateway.graphicsTree();
  // Measured on the unit: while a parameter editor (the Cab editor included) is open its tree has no zenUI::Grid at
  // all, so the Grid must not be required here; the editor itself is what is being closed.
  const screen = classifyQcGraphicsTree(tree);
  if (screen !== "parameter-editor" || !tree.includes("zenUI::ParameterEditor") || !stillCurrent()) {
    if (stillCurrent() && screen !== "grid") onLeftOpen?.(screen);
    return false;
  }
  await gateway.tapScreen(QC_PARAMETER_EDITOR_DONE.x, QC_PARAMETER_EDITOR_DONE.y);
  return true;
}
