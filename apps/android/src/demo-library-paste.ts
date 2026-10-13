/** A Demo paste belongs to the virtual session that opened its confirmation. */
export async function pasteOwnedDemoLibrary<Library, Snapshot>(options: {
  current(): boolean;
  readLibrary(): Promise<{ library: Library; confirmation: string } | undefined>;
  confirm(message: string): boolean | Promise<boolean>;
  replaceLibrary(library: Library): Promise<void>;
  readSnapshot(): Promise<Snapshot>;
  applied(snapshot: Snapshot): void;
}): Promise<void> {
  const document = await options.readLibrary();
  if (!options.current() || !document) return;
  const accepted = await options.confirm(document.confirmation);
  if (!options.current() || !accepted) return;
  await options.replaceLibrary(document.library);
  if (!options.current()) return;
  const snapshot = await options.readSnapshot();
  if (options.current()) options.applied(snapshot);
}
