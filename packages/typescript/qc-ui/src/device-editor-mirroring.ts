import type { GatewayTransport, GridBlock } from "@qc-remote/client";

// Native 800 x 480 Grid cell centres. Browser pixels must never be forwarded:
// their scale depends on the window and form factor.
export const QC_GRID_SCREEN_COLUMNS = [101, 187, 272, 357, 443, 529, 615, 701] as const;
export const QC_GRID_SCREEN_ROWS = [147, 241, 335, 429] as const;
export const QC_PARAMETER_EDITOR_DONE = { x: 742, y: 30 } as const;

function requireGrid(tree: string): void {
  if (!tree.includes("zenUI::Grid")) {
    throw new Error("The Quad Cortex is not showing the Grid. Return to the Grid, then select the block again.");
  }
}

function requireBareGrid(tree: string): void {
  requireGrid(tree);
  if (tree.includes("zenUI::ParameterEditor")) {
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

/** Close the physical parameter editor only when CorOS says it is visible. */
export async function closeGridBlockOnDevice(
  gateway: GatewayTransport,
  stillCurrent: () => boolean = () => true
): Promise<boolean> {
  const { tree } = await gateway.graphicsTree();
  requireGrid(tree);
  if (!tree.includes("zenUI::ParameterEditor") || !stillCurrent()) return false;
  await gateway.tapScreen(QC_PARAMETER_EDITOR_DONE.x, QC_PARAMETER_EDITOR_DONE.y);
  return true;
}
