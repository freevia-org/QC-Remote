type PendingSetter = (pending: boolean) => void;
const aliases = new WeakMap<PendingSetter, PendingSetter>();
const owners = new WeakMap<PendingSetter, object>();
const sessions = new WeakMap<PendingSetter, { key: unknown; managed: boolean }>();
function scope(setter: PendingSetter): PendingSetter {
  let next = aliases.get(setter);
  while (next && next !== setter) { setter = next; next = aliases.get(setter); }
  return setter;
}
/** Wrapped callbacks still share the host's one device-command slot. */
export function bindDeviceCommandScope(alias: PendingSetter, source: PendingSetter): void {
  if (aliases.has(alias)) return;
  const root = scope(source);
  if (alias !== root) aliases.set(alias, root);
}
export function deviceCommandBusy(setter: PendingSetter): boolean { return owners.has(scope(setter)); }
export function claimDeviceCommand(setter: PendingSetter, owner: object): boolean {
  const key = scope(setter), held = owners.get(key);
  if (held && held !== owner) return false;
  owners.set(key, owner);
  return true;
}
export function releaseDeviceCommand(setter: PendingSetter, owner: object): boolean {
  const key = scope(setter);
  if (owners.get(key) !== owner) return false;
  owners.delete(key);
  return true;
}

/** Callback remounts do not end a physical session; a changed host key does. */
export function registerDeviceCommandSession(setter: PendingSetter, key: unknown, managed = false): boolean {
  const root = scope(setter), previous = sessions.get(root);
  if (previous?.managed && !managed) return false;
  const changed = Boolean(previous && !Object.is(previous.key, key));
  if (changed) owners.delete(root);
  sessions.set(root, { key, managed: managed || previous?.managed === true });
  return changed;
}
/** A settled wire operation releases only its own claim, even after unmount. */
export function completeDeviceCommand(setter: PendingSetter, owner: object): void {
  const root = scope(setter);
  if (owners.get(root) !== owner) return;
  owners.delete(root);
  root(false);
}
