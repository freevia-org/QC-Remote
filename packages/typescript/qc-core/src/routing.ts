import { QC_INPUT_ROUTES, QC_OUTPUT_ROUTES, type PresetSnapshot } from "@qc-remote/client";

export type RouteSide = "input" | "output";
export type RouteOption = readonly [value: number, label: string];
export type RoutePickerGroup = "MONO" | "STEREO" | "OTHER" | "";
export type RouteDraft = { inputId: number; outputId: number; splitColumn: number | null; mixColumn: number | null };
export type RouteDrafts = Record<number, RouteDraft>;

export function routeDraft(route: PresetSnapshot["routes"][number]): RouteDraft {
  return {
    inputId: route.inputId ?? 0,
    outputId: route.outputId ?? 0,
    splitColumn: route.splitColumn ?? null,
    mixColumn: route.splitColumn === undefined ? null : route.mixColumn ?? -1
  };
}

export function routeDraftsFromSnapshot(snapshot: Pick<PresetSnapshot, "routes">): RouteDrafts {
  return Object.fromEntries(snapshot.routes
    .filter((route) => route.inputId !== undefined && route.outputId !== undefined)
    .map((route) => [route.row, routeDraft(route)]));
}

export function updateRouteDraft(current: RouteDrafts, route: PresetSnapshot["routes"][number], patch: Partial<RouteDraft>): RouteDrafts {
  return { ...current, [route.row]: { ...(current[route.row] ?? routeDraft(route)), ...patch } };
}

// CorOS presents ports in UI order, which intentionally differs from the wire enum.
export const inputRouteOptions: readonly RouteOption[] = [
  ...QC_INPUT_ROUTES.map(({ id, label }) => [id, label] as const)
];

export const outputRouteOptions: readonly RouteOption[] = [
  ...QC_OUTPUT_ROUTES.map(({ id, label }) => [id, label] as const)
];

export function routePickerLabel(side: RouteSide, label: string): string {
  if (label === "Internal") return "Not In Use";
  if (side === "output" && label === "Multi Out") return "Multiple Outputs";
  if (side === "input") {
    if (/^In /.test(label)) return label.replace(/^In /, "Input ");
    // CorOS is deliberately asymmetric here: input uses 'USB input 5', output uses 'USB Output 3'.
    if (/^USB /i.test(label)) return label.replace(/^USB /i, "USB input ");
  } else {
    if (/^Out /.test(label)) return label.replace(/^Out /, "Output ");
    if (/^USB output /i.test(label)) return label.replace(/^USB output /i, "USB Output ");
    if (/^USB /.test(label)) return label.replace(/^USB /, "USB Output ");
  }
  return label;
}

export function routePickerGroup(side: RouteSide, value: number): RoutePickerGroup {
  return (side === "input" ? QC_INPUT_ROUTES : QC_OUTPUT_ROUTES).find((route) => route.id === value)?.group ?? "";
}

export function routeOptionsForRow(side: RouteSide, row: number, currentValue: number, routes: PresetSnapshot["routes"]): readonly RouteOption[] {
  const allowed = new Set<number>();
  if (side === "input") {
    inputRouteOptions.forEach(([value]) => { if (value !== 7) allowed.add(value); });
    const rowOneOutput = routes.find((route) => route.row === 0)?.outputId;
    const rowTwoOutput = routes.find((route) => route.row === 1)?.outputId;
    if ((row === 2 && (rowOneOutput === 16 || rowOneOutput === 18)) || (row === 3 && (rowTwoOutput === 17 || rowOneOutput === 18))) allowed.add(7);
  } else {
    outputRouteOptions.forEach(([value]) => { if (value < 16 || value > 18) allowed.add(value); });
    if (row === 0) [16, 17, 18].forEach((value) => allowed.add(value));
    if (row === 1) allowed.add(17);
  }
  allowed.add(currentValue);
  return (side === "input" ? inputRouteOptions : outputRouteOptions).filter(([value]) => allowed.has(value));
}

export function routeOptionValue(side: RouteSide, id: number | undefined, label: string): number {
  if (id !== undefined) return id;
  const normalized = label.replaceAll(" ", "").toLowerCase();
  return (side === "input" ? inputRouteOptions : outputRouteOptions)
    .find(([, optionLabel]) => optionLabel.replaceAll(" ", "").toLowerCase() === normalized)?.[0] ?? 0;
}
