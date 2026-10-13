/**
 * The CorOS version number, from the two places the unit states it.
 *
 *  1. The host: the unit's update request (type 61) carries it in the
 *     `X-Cloud-Api-Hardware-Source-Version` header; the shared Rust session keeps it and the identity read reports it as
 *     `corOsVersion`. That value reaches the app only when it reads the identity.
 *  2. The unit's own Settings > Support > Device Information screen: while it is open, the graphics tree holds
 *     `zenUI::DeviceInformation` with consecutive text nodes `CorOS:` and then the version. The follower, which already
 *     reads the tree on both hosts, takes it from there (see `useQcScreenFollow`).
 *
 * Both are the same fact, so they normally agree. If they ever differ, the one obtained later wins
 * ({@link reconcileCorOsVersion}).
 *
 * The Device Information screen also shows the unit's serial number, MAC address, kernel and firmware hashes. This module
 * reads the one text node that follows the `CorOS:` label and nothing else, and keeps only a dotted version number.
 */

/** Two to four numeric parts of at most three ASCII digits each, the same shape the host accepts from the header. */
const DOTTED_VERSION = /^[0-9]{1,3}(?:\.[0-9]{1,3}){1,3}$/;

/** Whether `text` is a CorOS version number (surrounding blanks are ignored). */
export function isCorOsVersion(text: string | undefined): boolean {
  return text !== undefined && DOTTED_VERSION.test(text.trim());
}

const depthOf = (line: string) => line.length - line.trimStart().length;

/**
 * The CorOS version shown on the unit's Device Information screen, or `undefined` when the tree does not show that
 * screen, has no `CorOS:` label under it, or the text node right after the label is not a dotted version number.
 *
 * Only the node immediately after the label is looked at, so a different shape (a label with no value, a value that is
 * not a version) yields nothing rather than a later node, which on this screen could be the serial number.
 */
export function corOsVersionFromTree(tree: string): string | undefined {
  if (!tree.includes("zenUI::DeviceInformation")) return undefined;
  const lines = tree.split(/\r?\n/);
  const start = lines.findIndex((line) => /^\s*zenUI::DeviceInformation(?![A-Za-z0-9_])/.test(line));
  if (start < 0) return undefined;
  const base = depthOf(lines[start]);
  let afterLabel = false;
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index];
    // A class line at or above the screen's own depth ends its subtree.
    if (/^\s*zenUI::/.test(line) && depthOf(line) <= base) return undefined;
    if (!/^\s*text\s*:/.test(line)) continue;
    const text = line.match(/^\s*text\s*:\s*'(.*)'\s*$/)?.[1];
    if (afterLabel) return text !== undefined && isCorOsVersion(text) ? text.trim() : undefined;
    if (text?.trim() === "CorOS:") afterLabel = true;
  }
  return undefined;
}

/** A CorOS version and when (epoch milliseconds, this app's clock) it was obtained. */
export interface ObservedCorOsVersion { value: string; at: number }

/** What the app has gathered about the CorOS version this session. */
export interface CorOsVersionSightings {
  /** When the identity read that carried the host's value (the header-derived one) completed. */
  identityReadAt?: number;
  /** What the unit's Device Information screen last showed. */
  screen?: ObservedCorOsVersion;
}

/**
 * The CorOS version to show. The host's value is as current as the identity read that carried it, and the screen's is
 * as current as the last time the tree showed it: the later of the two wins, and on a tie the host's (the protocol
 * source) wins. When only one source has spoken, that one is shown; neither makes the caller show the reference version.
 */
export function reconcileCorOsVersion(hostValue: string | undefined, sightings: CorOsVersionSightings = {}): string | undefined {
  const host = hostValue !== undefined && isCorOsVersion(hostValue) ? hostValue.trim() : undefined;
  const screen = sightings.screen !== undefined && isCorOsVersion(sightings.screen.value) ? sightings.screen : undefined;
  if (host === undefined) return screen?.value.trim();
  if (screen === undefined) return host;
  return screen.at > (sightings.identityReadAt ?? 0) ? screen.value.trim() : host;
}
