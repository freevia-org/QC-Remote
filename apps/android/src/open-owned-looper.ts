/** A delayed Grid-block open may display only its accepted, still-owned view. */
export async function openOwnedLooper(options: {
  current(): boolean;
  open(): Promise<boolean>;
  show(): void;
}): Promise<void> {
  if (!options.current()) return;
  const accepted = await options.open();
  if (accepted && options.current()) options.show();
}
