// Static checks that the build manifests agree with each other.
//
// Each check below pins a fact that used to live only in a comment, or in two
// places that had already drifted apart:
//   - the Rust workspace lists every crate directory as a member, and shared
//     third-party crates are declared once, in [workspace.dependencies];
//   - the Android toolchain the build scripts accept is the one CI installs and
//     the README lists;
//   - type-only packages stay out of runtime dependencies, the Capacitor CLI
//     stays on a release without the vulnerable `xcode` dependency chain, and
//     the declared Node minimum can run the test scripts.
//
// There is no TOML or YAML parser among the root dependencies, so the readers
// are line-based and only understand the shapes these files actually use. An
// unrecognised shape fails rather than passing silently.

import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (relative) => readFileSync(path.join(repoRoot, relative), 'utf8').replace(/\r\n/g, '\n');
const readJson = (relative) => JSON.parse(read(relative));

// ---- Cargo manifests ---------------------------------------------------------

/** The `members = [...]` array of the root manifest's [workspace] table. */
export function workspaceMembers(rootManifest) {
  const block = /^members\s*=\s*\[([^\]]*)\]/m.exec(rootManifest);
  assert.ok(block, 'root Cargo.toml has no members array');
  return [...block[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

/**
 * Every dependency line of a manifest, with the section it sits in. Covers
 * `name = "1"`, `name = { ... }` (single or multi-line) and
 * `name.workspace = true`.
 */
export function dependencyEntries(manifest) {
  const entries = [];
  let section = '';
  for (const line of manifest.split('\n')) {
    const header = /^\[([^\]]+)\]\s*$/.exec(line);
    if (header) {
      section = header[1];
      continue;
    }
    if (!/(^|\.)(dev-|build-)?dependencies$/.test(section)) continue;
    if (/^\s*(#|$)/.test(line) || /^\s+/.test(line) || /^\]/.test(line)) continue;
    const match = /^([A-Za-z0-9_-]+)(\.workspace)?\s*=\s*(.+)$/.exec(line);
    assert.ok(match, `unrecognised dependency line in [${section}]: ${line}`);
    const [, name, dottedWorkspace, value] = match;
    const inherited = Boolean(dottedWorkspace) || /\bworkspace\s*=\s*true\b/.test(value);
    const version = /^"([^"]+)"/.exec(value)?.[1] ?? /\bversion\s*=\s*"([^"]+)"/.exec(value)?.[1] ?? null;
    const isPath = /\bpath\s*=/.test(value);
    entries.push({ section, name, inherited, version, isPath, line });
  }
  return entries;
}

/** The [workspace.dependencies] table as name -> version. */
export function workspaceDependencies(rootManifest) {
  const table = /^\[workspace\.dependencies\]\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(rootManifest);
  assert.ok(table, 'root Cargo.toml has no [workspace.dependencies] table');
  const versions = new Map();
  for (const entry of dependencyEntries(`[dependencies]\n${table[1]}`)) {
    assert.ok(entry.version, `[workspace.dependencies] ${entry.name} has no version`);
    versions.set(entry.name, entry.version);
  }
  return versions;
}

const rootManifest = read('Cargo.toml');
const members = workspaceMembers(rootManifest);
const memberManifests = new Map(members.map((member) => [member, read(`${member}/Cargo.toml`)]));
const shared = workspaceDependencies(rootManifest);

test('the workspace member list is every crate directory, explicitly', () => {
  const crateDirectories = ['packages/rust', 'services'].flatMap((parent) =>
    readdirSync(path.join(repoRoot, parent), { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && existsSync(path.join(repoRoot, parent, entry.name, 'Cargo.toml')))
      .map((entry) => `${parent}/${entry.name}`),
  );
  assert.deepEqual([...members].sort(), crateDirectories.sort());
});

test('members inherit every crate [workspace.dependencies] declares', () => {
  const restated = [];
  for (const [member, manifest] of memberManifests) {
    for (const entry of dependencyEntries(manifest)) {
      if (shared.has(entry.name) && !entry.inherited) restated.push(`${member}: ${entry.line.trim()}`);
    }
  }
  assert.deepEqual(restated, [], 'use `name.workspace = true` (plus any member-only features) instead of a literal version');
});

test('a third-party crate used by two members is declared once, in [workspace.dependencies]', () => {
  const users = new Map();
  for (const [member, manifest] of memberManifests) {
    for (const entry of dependencyEntries(manifest)) {
      if (entry.isPath) continue;
      users.set(entry.name, new Set([...(users.get(entry.name) ?? []), member]));
    }
  }
  const undeclared = [...users].filter(([name, by]) => by.size > 1 && !shared.has(name)).map(([name, by]) => `${name} (${[...by].join(', ')})`);
  assert.deepEqual(undeclared, []);
});

test('the Windows shell, outside the workspace, names the same versions of the shared crates', () => {
  // apps/windows/src-tauri is its own workspace and cannot inherit; a version
  // split there would link two copies of a crate into one binary through the
  // path dependencies (services/device-broker's comment on `rand` names one
  // reason this matters).
  const mismatched = dependencyEntries(read('apps/windows/src-tauri/Cargo.toml'))
    .filter((entry) => shared.has(entry.name) && entry.version !== shared.get(entry.name))
    .map((entry) => `${entry.name}: shell ${entry.version}, workspace ${shared.get(entry.name)}`);
  assert.deepEqual(mismatched, []);
});

test('the dependency-line reader rejects shapes it does not understand', () => {
  assert.throws(() => dependencyEntries('[dependencies]\n"quoted" = "1"\n'), /unrecognised dependency line/);
  assert.deepEqual(
    dependencyEntries('[target.\'cfg(windows)\'.dependencies]\nserde.workspace = true\nrand = "0.9"\nwindows-sys = { workspace = true, features = [\n    "A",\n] }\n').map(
      ({ name, inherited, version }) => [name, inherited, version],
    ),
    [['serde', true, null], ['rand', false, '0.9'], ['windows-sys', true, null]],
  );
});

// ---- Android toolchain -------------------------------------------------------

function single(pattern, text, what) {
  const matches = [...text.matchAll(new RegExp(pattern, 'g'))].map((match) => match[1]);
  assert.ok(matches.length > 0, `no ${what} found`);
  assert.equal(new Set(matches).size, 1, `${what} disagrees with itself: ${[...new Set(matches)].join(', ')}`);
  return matches[0];
}

const ci = read('.github/workflows/ci.yml');
const readme = read('README.md');
const rustScript = read('scripts/build-android-rust.ps1');
const debugScript = read('scripts/build-android-debug.ps1');

test('the Android native build accepts exactly the cargo-ndk and NDK that CI installs', () => {
  const cargoNdk = single(/cargo install cargo-ndk --version (\S+) --locked/.source, ci, 'CI cargo-ndk version');
  const ndk = single(/ndk;(\d+\.\d+\.\d+)/.source, ci, 'CI NDK package');
  assert.equal(single(/\$requiredCargoNdkVersion = "([^"]+)"/.source, rustScript, 'script cargo-ndk pin'), cargoNdk);
  assert.equal(single(/\$requiredNdkVersion = "([^"]+)"/.source, rustScript, 'script NDK pin'), ndk);
  assert.equal(single(/cargo install cargo-ndk --version (\S+) --locked/.source, readme, 'README cargo-ndk version'), cargoNdk);
  assert.ok(readme.includes(`| Android NDK | ${ndk}`), `README does not list NDK ${ndk}`);
  // The old script took the lexically highest ndk/* directory.
  assert.doesNotMatch(rustScript, /Sort-Object Name -Descending/);
});

test('the Android debug build accepts exactly the JDK that CI and Gradle use', () => {
  const jdk = single(/java-version: '(\d+)'/.source, ci, 'CI java-version');
  assert.equal(single(/\$requiredJdkMajor = "(\d+)"/.source, debugScript, 'script JDK pin'), jdk);
  assert.match(debugScript, /-match "\^\$requiredJdkMajor\\\."/);
  assert.doesNotMatch(debugScript, /jdk-17|\b17\|21\b/);
  const gradle = read('apps/android/android/app/capacitor.build.gradle');
  assert.equal(single(/JavaVersion\.VERSION_(\d+)/.source, gradle, 'Gradle Java version'), jdk);
  assert.ok(readme.includes(`| JDK | ${jdk} `), `README does not list JDK ${jdk}`);
});

// ---- npm manifests -----------------------------------------------------------

const rootPackage = readJson('package.json');
const lock = readJson('package-lock.json');
const workspacePackages = Object.keys(lock.packages).filter((key) => key !== '' && !key.includes('node_modules/'));

test('type-only packages are never runtime dependencies', () => {
  const misplaced = workspacePackages.flatMap((dir) =>
    Object.keys(readJson(`${dir}/package.json`).dependencies ?? {})
      .filter((name) => name.startsWith('@types/'))
      .map((name) => `${dir}: ${name}`),
  );
  assert.deepEqual(misplaced, []);
});

test('the Capacitor CLI stays on a release without the xcode dependency chain', () => {
  // @capacitor/cli 8.5 depends on `xcode`, which brings an old `uuid` (^7)
  // and `simple-plist`; that chain was removed from the lock as vulnerable.
  // 8.4.3 does not depend on `xcode`, so the CLI is pinned exactly while @capacitor/core and
  // @capacitor/android float on ^8.5. Lift the pin only to a CLI release whose
  // locked entry below no longer depends on `xcode`.
  const spec = readJson('apps/android/package.json').devDependencies['@capacitor/cli'];
  assert.match(spec, /^\d+\.\d+\.\d+$/, '@capacitor/cli must stay an exact version');
  assert.equal(readJson('apps/ios/package.json').devDependencies['@capacitor/cli'], spec);
  const lockedCopies = Object.entries(lock.packages)
    .filter(([path]) => path === 'node_modules/@capacitor/cli' || path.endsWith('/node_modules/@capacitor/cli'))
    .map(([, entry]) => entry);
  assert.ok(lockedCopies.length > 0, '@capacitor/cli is missing from the lockfile');
  for (const locked of lockedCopies) {
    assert.equal(locked.version, spec, `@capacitor/cli ${locked.version} does not match ${spec}`);
    assert.ok(!('xcode' in (locked.dependencies ?? {})), `@capacitor/cli ${locked.version} depends on xcode`);
  }
  assert.ok(!lock.packages['node_modules/xcode'], 'xcode is in the lockfile');
});

test('the declared Node minimum runs the test scripts', () => {
  // --experimental-strip-types first shipped in Node 22.6.0.
  const usesStripTypes = workspacePackages.some((dir) =>
    Object.values(readJson(`${dir}/package.json`).scripts ?? {}).some((script) => script.includes('--experimental-strip-types')),
  );
  assert.ok(usesStripTypes, 'no script uses --experimental-strip-types; revisit this check');
  const minimum = /^>=(\d+)(?:\.(\d+))?/.exec(rootPackage.engines?.node ?? '');
  assert.ok(minimum, `unrecognised engines.node ${rootPackage.engines?.node}`);
  const [major, minor] = [Number(minimum[1]), Number(minimum[2] ?? 0)];
  assert.ok(major > 22 || (major === 22 && minor >= 6), `engines.node ${rootPackage.engines.node} is below 22.6`);
  assert.equal(lock.packages[''].engines?.node, rootPackage.engines.node, 'package-lock.json root engines is stale');
});
