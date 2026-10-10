import { QC_GRID_COLUMNS, QC_GRID_ROWS, type BlockDetails, type PresetSnapshot } from "@qc-remote/client";
import { sceneLetter } from "@qc-remote/core";

export interface GridManagementPanelProps {
  snapshot: PresetSnapshot;
  details?: BlockDetails;
  loading: boolean;
  pending: boolean;
  moveDestination?: number;
  setMoveDestination(value?: number): void;
  /** Destination row; undefined keeps the block's own row. */
  moveDestinationRow?: number;
  setMoveDestinationRow(value?: number): void;
  footswitchDraft: number | null;
  setFootswitchDraft(value: number | null): void;
  move(): void;
  assignFootswitch(): void;
  remove(): void;
}

export function GridManagementPanel(props: GridManagementPanelProps) {
  const { snapshot, details, loading, pending, moveDestination, setMoveDestination, moveDestinationRow, setMoveDestinationRow, footswitchDraft, setFootswitchDraft, move, assignFootswitch, remove } = props;
  // The picker reaches every cell a Grid drag can: any empty cell in any row.
  const destinationRow = moveDestinationRow ?? details?.row ?? 0;
  const emptyColumns = details ? Array.from({ length: QC_GRID_COLUMNS }, (_, column) => column).filter((column) => !(destinationRow === details.row && column === details.column) && !snapshot.blocks.some((block) => block.row === destinationRow && block.column === column)) : [];
  return <>
    <div className="dialog-kicker">BLOCK EDITOR · SCENE {sceneLetter(snapshot.activeScene)}</div>
    <h2 id="dialog-title">{details?.name ?? "Loading block…"}</h2>
    {loading ? <p>Reading parameter metadata and live values…</p> : details && <div className="block-management">
      <label><span>Move to an empty cell<small>From row {details.row + 1}, column {details.column + 1}. Only empty cells are offered; each row keeps its own routing.</small></span>
        <span className="block-move-destination">
          <select aria-label="Destination row" value={destinationRow} disabled={pending} onChange={(event) => { setMoveDestinationRow(Number(event.target.value)); setMoveDestination(undefined); }}>
            {Array.from({ length: QC_GRID_ROWS }, (_, row) => <option value={row} key={row}>Row {row + 1}</option>)}
          </select>
          <select aria-label="Destination column" value={moveDestination ?? ""} disabled={pending} onChange={(event) => setMoveDestination(event.target.value === "" ? undefined : Number(event.target.value))}>
            <option value="">{emptyColumns.length ? "Choose empty column…" : "No empty column"}</option>
            {emptyColumns.map((column) => <option value={column} key={column}>Column {column + 1}</option>)}
          </select>
        </span>
        <button disabled={moveDestination === undefined || pending} onClick={move}>Move</button>
      </label>
      <label><span>STOMP footswitch<small>Several blocks may share the same switch.</small></span>
        <select value={footswitchDraft ?? ""} disabled={pending} onChange={(event) => setFootswitchDraft(event.target.value === "" ? null : Number(event.target.value))}>
          <option value="">Unassigned</option>
          {Array.from({ length: 8 }, (_, index) => <option value={index} key={index}>Footswitch {sceneLetter(index)}</option>)}
        </select>
        <button disabled={footswitchDraft === (snapshot.blocks.find((block) => block.row === details.row && block.column === details.column)?.footswitch ?? null) || pending} onClick={assignFootswitch}>Review assignment…</button>
      </label>
      <div className="block-management-actions"><span>Remove this block<small>Discard Unsaved Changes restores the stored preset.</small></span><button className="danger" disabled={pending} onClick={remove}>Review removal…</button></div>
    </div>}
    <p>Changes apply temporarily to the live Grid and require a separate preset save to persist.</p>
  </>;
}
