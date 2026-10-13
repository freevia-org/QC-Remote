type PendingSetter = (pending: boolean) => void;
const aliases = new WeakMap<PendingSetter, PendingSetter>();
const owners = new WeakMap<PendingSetter, object>();
function scope(setter: PendingSetter): PendingSetter {
  let next = aliases.get(setter);
  while (next && next !== setter) { setter = next; next = aliases.get(setter); }
  return setter;
}
/** Wrapped callbacks still share the host's one device-command slot. */
export function bindDeviceCommandScope(alias: PendingSetter, source: PendingSetter): void {
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
