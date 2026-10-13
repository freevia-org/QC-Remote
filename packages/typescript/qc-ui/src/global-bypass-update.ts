type BypassRows = { row1?: boolean; row2?: boolean; row3?: boolean; row4?: boolean };
type BypassMask = [boolean, boolean, boolean, boolean];

/** Whole-mask writes preserve every observed row and leave the other category untouched. */
export function observedGlobalBypassUpdate(
  settings: { globalBypassCab?: BypassRows; globalBypassIr?: BypassRows } | undefined,
  category: "cab" | "ir", row: 1 | 2 | 3 | 4, bypassed: boolean
): { cab: BypassMask | null; ir: BypassMask | null } | undefined {
  const observed = category === "cab" ? settings?.globalBypassCab : settings?.globalBypassIr;
  if (!observed || ![observed.row1, observed.row2, observed.row3, observed.row4].every((value) => typeof value === "boolean")) return;
  const rows: BypassMask = [observed.row1!, observed.row2!, observed.row3!, observed.row4!];
  rows[row - 1] = bypassed;
  return category === "cab" ? { cab: rows, ir: null } : { cab: null, ir: rows };
}
