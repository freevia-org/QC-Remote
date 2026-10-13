/** A device refresh cannot publish after its USB session or preset is replaced. */
export async function refreshOwnedNativeSnapshot<Snapshot>(options: {
  current(): boolean;
  read(): Promise<Snapshot>;
  apply(snapshot: Snapshot): void;
}): Promise<void> {
  const snapshot = await options.read();
  if (options.current()) options.apply(snapshot);
}
