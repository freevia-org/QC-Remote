import { SHARED_QC_ACCESS_MODES, type ControlAccessMode } from "@qc-remote/core";

const accessLabels: Record<ControlAccessMode, string> = {
  "read-only": "Read-only", performance: "Performance", modify: "Modify", full: "Full control"
};
export const CONTROL_ACCESS_OPTIONS: ReadonlyArray<{ value: ControlAccessMode; label: string }> =
  SHARED_QC_ACCESS_MODES.map((value) => ({ value, label: accessLabels[value] }));

export function ControlAccessSelect(props: {
  value: ControlAccessMode;
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  onChange: (value: ControlAccessMode) => void;
}) {
  return <select className={props.className} value={props.value} disabled={props.disabled} aria-label={props.ariaLabel ?? "MCP device access"} onChange={(event) => props.onChange(event.target.value as ControlAccessMode)}>
    {CONTROL_ACCESS_OPTIONS.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}
  </select>;
}
