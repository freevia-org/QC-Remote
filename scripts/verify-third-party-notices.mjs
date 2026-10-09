#!/usr/bin/env node
// Fails when the third-party legal record drifts from the locked dependency
// graphs: THIRD_PARTY-NOTICES.md, legal/THIRD_PARTY-LICENSE-INVENTORY.json,
// legal/THIRD_PARTY-LICENSE-TEXTS.txt and legal/THIRD_PARTY-SOURCE-OFFER.md.
//
// Four lockfiles describe the dependency graphs:
//   - the root Cargo workspace's Cargo.lock
//   - apps/windows/src-tauri/Cargo.lock, a *separate* lockfile because
//     apps/windows/src-tauri is excluded from the root workspace (see the
//     `exclude` entry in Cargo.toml)
//   - package-lock.json (npm workspaces)
//   - apps/android/android/app/gradle.lockfile, the Android app's locked
//     Maven runtime classpath. A lockfile carries no license, so each library's
//     license is the one its Maven POM declares, as recorded in the inventory.
//
// Shipped and development tooling are told apart the same way the inventory
// generator tells them apart:
//   - Rust: the crates reachable over normal and build edges from the three
//     artefacts that ship - the Windows app (apps/windows/src-tauri), the
//     device broker (services/device-broker), both for x86_64-pc-windows-msvc,
//     and the Android library (packages/rust/quadcortex-android) for
//     aarch64-linux-android. Dev-dependencies and other platforms' crates do
//     not ship.
//   - npm: the packages reachable over `dependencies`, `optionalDependencies`
//     and `peerDependencies` from the two app workspaces (apps/windows,
//     apps/android). Build tools such as vite, typescript, wrangler or sharp are
//     development tooling, as is everything only the Cloudflare relay uses (the
//     relay runs as a hosted service and is not distributed).
//   - Android: the exact locked releaseRuntimeClasspath (unflavored release).
//
// Checks:
//   1. THIRD_PARTY-NOTICES.md lists every shipped component under "Shipped
//      components" and other locked Rust/npm packages under
//      "Development tooling", by name and normalized SPDX license.
//   2. The inventory records exactly the shipped Rust crates and npm packages,
//      version for version (name@version), with the license each declares; and
//      exactly the locked Android runtime libraries.
//   3. The source offer lists exactly the inventory's copyleft components, each
//      with its exact-version archive, and nothing the shipped graph lacks.
//   4. Every license-text hash the inventory names is a record in
//      THIRD_PARTY-LICENSE-TEXTS.txt, every record there is in the inventory,
//      and each digest matches its LF-normalized bundled body.
//
// The inventory, texts and source offer are produced by the maintainer's
// inventory generator; this script only verifies them. The notices file is
// regenerated here.
//
// Check directly: node scripts/verify-third-party-notices.mjs
// Regenerate:     node scripts/verify-third-party-notices.mjs --write

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

export const ANDROID_LOCKFILE = 'apps/android/android/app/gradle.lockfile';
export const LICENSE_INVENTORY = 'legal/THIRD_PARTY-LICENSE-INVENTORY.json';
export const LICENSE_TEXTS = 'legal/THIRD_PARTY-LICENSE-TEXTS.txt';
export const SOURCE_OFFER = 'legal/THIRD_PARTY-SOURCE-OFFER.md';

/** The artefacts that ship, as the inventory generator defines them. */
export const SHIPPED_CARGO_ROOTS = [
  { manifest: 'apps/windows/src-tauri/Cargo.toml', target: 'x86_64-pc-windows-msvc' },
  { manifest: 'services/device-broker/Cargo.toml', target: 'x86_64-pc-windows-msvc' },
  { manifest: 'packages/rust/quadcortex-android/Cargo.toml', target: 'aarch64-linux-android' },
];
export const SHIPPED_NPM_ROOTS = ['apps/windows', 'apps/android'];
const ALL_CARGO_MANIFESTS = ['Cargo.toml', 'apps/windows/src-tauri/Cargo.toml'];

/** Licenses that raise a source-availability obligation for a distributed binary. */
export const COPYLEFT = /MPL-2\.0|GPL|LGPL|AGPL/i;

export const SHIPPED_SECTION = '## Shipped components';
export const DEVELOPMENT_SECTION = '## Development tooling (not distributed)';
export const RUST_HEADING = '### Rust crates';
export const NPM_HEADING = '### npm packages';
export const ANDROID_HEADING = '### Android libraries (Maven)';

// ---------------------------------------------------------------------------
// License normalization

// Longest first, so a name never matches inside a longer one.
const PROSE_LICENSES = [
  [/gnu general public license, version 2 \(gpl2\), with the classpath exception/gi, 'GPL-2.0-only WITH Classpath-exception-2.0'],
  [/the apache software license,? version 2\.0/gi, 'Apache-2.0'],
  [/(?:the )?apache license,? version 2\.0/gi, 'Apache-2.0'],
  [/\bapache 2\.0\b/gi, 'Apache-2.0'],
  [/(?:the )?mit license/gi, 'MIT'],
];

/**
 * One SPDX spelling per license expression, so the same choice is one heading:
 * the license names Maven POMs use are mapped to their SPDX identifiers, `/`
 * is SPDX `OR`, and the operands of a plain disjunction (no AND) are sorted,
 * including each parenthesized plain choice inside a compound.
 */
export function normalizeLicense(expression) {
  let text = String(expression ?? '').trim();
  for (const [pattern, identifier] of PROSE_LICENSES) text = text.replace(pattern, identifier);
  text = text.replace(/\s*\/\s*/g, ' OR ').replace(/\s+/g, ' ');
  if (/[()]/.test(text) || /\bAND\b/.test(text)) {
    return text.replace(/\(([^()]+)\)/g, (group, inner) => (/\bAND\b/.test(inner) ? group : `(${normalizeLicense(inner)})`));
  }
  const operands = text.split(/ OR /).map((operand) => operand.trim());
  return [...new Set(operands)].sort((left, right) => (left < right ? -1 : left > right ? 1 : 0)).join(' OR ');
}

// ---------------------------------------------------------------------------
// Dependency graphs

function addLicense(map, name, license) {
  if (!map.has(name)) map.set(name, new Set());
  map.get(name).add(normalizeLicense(license));
}

function cargoMetadata(manifestPath, target) {
  const args = ['metadata', '--format-version', '1', '--locked', '--manifest-path', manifestPath];
  if (target) args.push('--filter-platform', target);
  const out = execFileSync('cargo', args, { cwd: repoRoot, maxBuffer: 1024 * 1024 * 64, encoding: 'utf8' });
  return JSON.parse(out);
}

/**
 * The third-party packages reachable from one root over normal and build
 * edges, as Map<"name@version", { name, version, license }>. `rootManifest`
 * is compared after path.resolve against each package's manifest_path.
 */
export function shippedCargoPackages(metadata, rootManifest) {
  const packages = new Map((metadata.packages ?? []).map((item) => [item.id, item]));
  const root = (metadata.packages ?? []).find((item) => path.resolve(item.manifest_path) === path.resolve(rootManifest));
  if (!root) throw new Error(`cargo metadata has no package for ${rootManifest}`);
  const edges = new Map((metadata.resolve?.nodes ?? []).map((node) => [node.id, node.deps ?? []]));
  const shipping = (dependency) => (dependency.dep_kinds ?? []).some((kind) => kind.kind === null || kind.kind === undefined || kind.kind === 'build');
  const reachable = new Set();
  const queue = [root.id];
  while (queue.length) {
    const id = queue.pop();
    if (reachable.has(id)) continue;
    reachable.add(id);
    for (const dependency of edges.get(id) ?? []) if (shipping(dependency)) queue.push(dependency.pkg);
  }
  const shipped = new Map();
  for (const id of reachable) {
    const item = packages.get(id);
    if (!item || item.source === null || item.source === undefined) continue; // this repository's own crates
    shipped.set(`${item.name}@${item.version}`, { name: item.name, version: item.version, license: item.license ?? null });
  }
  return shipped;
}

/** Resolve `name` from lock location `from` the way Node resolves a bare import. */
function resolveLockedPackage(packages, from, name) {
  let directory = from;
  for (;;) {
    const key = `${directory ? `${directory}/` : ''}node_modules/${name}`;
    if (packages[key]) return key;
    if (!directory) return null;
    const nested = directory.lastIndexOf('/node_modules/');
    if (nested !== -1) directory = directory.slice(0, nested);
    else if (directory.startsWith('node_modules/')) directory = '';
    else directory = directory.includes('/') ? directory.slice(0, directory.lastIndexOf('/')) : '';
  }
}

const lockName = (key, entry) => entry.name ?? key.slice(key.lastIndexOf('node_modules/') + 'node_modules/'.length);

/**
 * The third-party npm packages the app workspaces ship, as
 * Map<"name@version", { name, version, license }>: everything reachable from
 * `roots` over runtime dependency edges in package-lock.json, following
 * workspace links but not counting this repository's own packages.
 */
export function shippedNpmPackages(lock, roots = SHIPPED_NPM_ROOTS) {
  const packages = lock.packages ?? {};
  const seen = new Set();
  const queue = [...roots];
  while (queue.length) {
    const location = queue.pop();
    if (seen.has(location)) continue;
    seen.add(location);
    const entry = packages[location];
    if (!entry) continue;
    if (entry.link === true) { queue.push(entry.resolved); continue; }
    for (const field of ['dependencies', 'optionalDependencies', 'peerDependencies']) {
      for (const name of Object.keys(entry[field] ?? {})) {
        const resolved = resolveLockedPackage(packages, location, name);
        if (resolved) queue.push(resolved);
      }
    }
  }
  const shipped = new Map();
  for (const location of seen) {
    if (!location.includes('node_modules/')) continue;
    const entry = packages[location];
    if (!entry || entry.link === true || !entry.version) continue;
    const name = lockName(location, entry);
    shipped.set(`${name}@${entry.version}`, { name, version: entry.version, license: entry.license ?? null });
  }
  return shipped;
}

/** Every third-party npm package in package-lock.json, as Map<name, Set<license>>. */
export function allNpmPackages(lock) {
  const deps = new Map();
  const noLicense = [];
  for (const [key, entry] of Object.entries(lock.packages ?? {})) {
    if (!key.includes('node_modules/')) continue; // the root and workspace member manifests
    if (entry.link === true) continue; // a link to one of this repository's own workspace packages
    const name = lockName(key, entry);
    if (!entry.license) { noLicense.push(`${name} ${entry.version ?? '(unknown version)'}`); continue; }
    addLicense(deps, name, entry.license);
  }
  return { deps, noLicense };
}

/** Every third-party crate the two Cargo lockfiles resolve, for every platform, as Map<name, Set<license>>. */
function allCargoPackages(metadataSets) {
  const deps = new Map();
  const noLicense = [];
  for (const { metadata, manifest } of metadataSets) {
    for (const item of metadata.packages) {
      if (item.source === null) continue;
      if (!item.license) { noLicense.push(`${item.name} ${item.version} (${manifest})`); continue; }
      addLicense(deps, item.name, item.license);
    }
  }
  return { deps, noLicense };
}

/** The shipped Android release runtime classpath, as a Set of "group:artifact@version". */
export function androidRuntimeLibraries(lockfile) {
  const locked = new Set();
  for (const line of lockfile.split(/\r?\n/)) {
    const entry = /^([^#=\s:]+):([^=\s:]+):([^=\s:]+)=(.*)$/.exec(line.trim());
    if (!entry) continue;
    const [, group, artifact, version, configurations] = entry;
    if (!configurations.split(',').some((configuration) => configuration.trim() === 'releaseRuntimeClasspath')) continue;
    locked.add(`${group}:${artifact}@${version}`);
  }
  return locked;
}

const byName = (shipped) => {
  const deps = new Map();
  for (const { name, license } of shipped.values()) if (license) addLicense(deps, name, license);
  return deps;
};

// ---------------------------------------------------------------------------
// Inventory, source offer and license texts

const componentKey = (item) => `${item.name}@${item.version}`;

/**
 * The inventory must record exactly what ships, version for version: every
 * shipped crate and npm package at its locked version with the license it
 * declares, and every locked Android runtime library. A record of anything
 * else is stale.
 */
export function inventoryErrors(inventory, { cargo, npm, android }) {
  const errors = [];
  const compare = (ecosystem, shipped, label) => {
    const recorded = new Map((inventory.components ?? []).filter((item) => item.ecosystem === ecosystem).map((item) => [componentKey(item), item]));
    for (const [key, item] of shipped) {
      const record = recorded.get(key);
      if (!record) { errors.push(`${LICENSE_INVENTORY} does not record ${ecosystem}:${key}, which ${label} ships`); continue; }
      if (item.license && record.license !== item.license) errors.push(`${LICENSE_INVENTORY} records ${ecosystem}:${key} as ${record.license}, but it declares ${item.license}`);
    }
    for (const key of recorded.keys()) if (!shipped.has(key)) errors.push(`${LICENSE_INVENTORY} records ${ecosystem}:${key}, which ${label} does not ship at that version`);
  };
  compare('cargo', cargo, 'the locked Rust graph (Cargo.lock + apps/windows/src-tauri/Cargo.lock)');
  compare('npm', npm, 'the locked app workspaces (package-lock.json)');
  compare('gradle', new Map([...android].map((key) => [key, { license: null }])), `the locked Android runtime classpath (${ANDROID_LOCKFILE})`);
  return errors;
}

/** `| component | archive | upstream |` rows of the source offer's table. */
export function sourceOfferRows(markdown) {
  const rows = [];
  for (const line of markdown.split(/\r?\n/)) {
    const cells = /^\|\s*([a-z]+:[^|\s]+@[^|\s]+)\s*\|\s*([^|]*?)\s*\|\s*([^|]*?)\s*\|$/.exec(line.trim());
    if (cells) rows.push({ component: cells[1], sourceArchiveUrl: cells[2], sourceUrl: cells[3] });
  }
  return rows;
}

/**
 * The source offer lists exactly the inventory's copyleft components, each
 * with the archive of its exact version, and states which components it
 * covers and why the rest are absent.
 */
export function sourceOfferErrors(inventory, markdown) {
  const errors = [];
  const rows = sourceOfferRows(markdown);
  const components = new Map((inventory.components ?? []).map((item) => [`${item.ecosystem}:${componentKey(item)}`, item]));
  const copyleft = [...components].filter(([, item]) => COPYLEFT.test(item.selectedLicense ?? item.license ?? '')).map(([key]) => key).sort();
  const notice = (inventory.sourceAvailabilityNotice ?? []).map((item) => item.component).sort();
  if (JSON.stringify(notice) !== JSON.stringify(copyleft)) errors.push(`${LICENSE_INVENTORY} sourceAvailabilityNotice lists ${notice.join(', ') || 'nothing'}, but its copyleft components are ${copyleft.join(', ') || 'none'}`);
  const listed = rows.map((row) => row.component).sort();
  if (JSON.stringify(listed) !== JSON.stringify(copyleft)) errors.push(`${SOURCE_OFFER} lists ${listed.join(', ') || 'nothing'}, but the shipped copyleft components are ${copyleft.join(', ') || 'none'}`);
  for (const row of rows) {
    const item = components.get(row.component);
    if (!item) { errors.push(`${SOURCE_OFFER} lists ${row.component}, which ${LICENSE_INVENTORY} does not record`); continue; }
    const archive = item.sourceArchiveUrl ?? null;
    if (row.sourceArchiveUrl !== archive) errors.push(`${SOURCE_OFFER} gives ${row.sourceArchiveUrl} for ${row.component}; the inventory records ${archive}`);
    if (item.ecosystem === 'cargo' && !row.sourceArchiveUrl.endsWith(`/crates/${encodeURIComponent(item.name)}/${encodeURIComponent(item.version)}/download`)) errors.push(`${SOURCE_OFFER} archive for ${row.component} is not that exact version`);
  }
  if (!/selection rule/i.test(markdown)) errors.push(`${SOURCE_OFFER} does not state its selection rule (which components it covers, and that development tooling is not distributed)`);
  return errors;
}

/** Every text hash the inventory names is a record of the texts file, and the other way round. */
export function licenseTextErrors(inventory, texts) {
  const errors = [];
  const normalized = texts.replace(/\r\n/g, '\n');
  const inBundle = new Set([...normalized.matchAll(/^SHA-256: ([0-9a-f]{64})$/gm)].map((match) => match[1]));
  const records = normalized.split(/^={80}\n(?=SHA-256: [0-9a-f]{64}\n)/m).slice(1);
  const verified = new Set();
  for (const record of records) {
    const hash = /^SHA-256: ([0-9a-f]{64})\n/.exec(record)?.[1];
    const divider = record.indexOf(`${'-'.repeat(80)}\n`);
    if (!hash || divider < 0) { errors.push(`${LICENSE_TEXTS} contains a malformed license text record`); continue; }
    const body = record.slice(divider + 81).trim();
    const actual = createHash('sha256').update(body).digest('hex');
    if (actual !== hash) errors.push(`${LICENSE_TEXTS} license text ${hash} body hashes to ${actual}`);
    if (verified.has(hash)) errors.push(`${LICENSE_TEXTS} repeats license text ${hash}`);
    verified.add(hash);
  }
  for (const hash of inBundle) if (!verified.has(hash)) errors.push(`${LICENSE_TEXTS} has no body record for ${hash}`);
  const rows = Array.isArray(inventory.components) ? inventory.components : [];
  const byHash = new Map();
  const missing = [];
  for (const row of rows) {
    const key = `${row.ecosystem}:${componentKey(row)}`;
    const hashes = Array.isArray(row.licenseTextHashes) ? row.licenseTextHashes : [];
    if (hashes.length === 0) {
      missing.push(key);
      errors.push(`${key} has no associated license text`);
    }
    for (const hash of hashes) {
      if (typeof hash !== 'string' || !/^[0-9a-f]{64}$/.test(hash)) {
        errors.push(`${key} has an invalid license text hash`);
        continue;
      }
      const linked = byHash.get(hash) ?? new Set();
      linked.add(key);
      byHash.set(hash, linked);
    }
  }
  const missingRecorded = Array.isArray(inventory.missingLocalLicenseText) ? [...inventory.missingLocalLicenseText].sort() : [];
  if (JSON.stringify([...missing].sort()) !== JSON.stringify(missingRecorded)) {
    errors.push(`${LICENSE_INVENTORY} missingLocalLicenseText does not match components without license text`);
  }
  const inventoryRecords = Array.isArray(inventory.licenseTexts) ? inventory.licenseTexts : [];
  const declared = new Map();
  for (const item of inventoryRecords) {
    if (declared.has(item.sha256)) errors.push(`${LICENSE_INVENTORY} repeats license text ${item.sha256}`);
    declared.set(item.sha256, item);
  }
  const named = new Set(byHash.keys());
  for (const hash of named) if (!inBundle.has(hash)) errors.push(`${LICENSE_INVENTORY} names license text ${hash}, which ${LICENSE_TEXTS} does not contain`);
  for (const hash of declared.keys()) if (!inBundle.has(hash)) errors.push(`${LICENSE_INVENTORY} licenseTexts lists ${hash}, which ${LICENSE_TEXTS} does not contain`);
  for (const hash of inBundle) if (!declared.has(hash)) errors.push(`${LICENSE_TEXTS} holds text ${hash}, which ${LICENSE_INVENTORY} does not list`);
  for (const hash of named) if (!declared.has(hash)) errors.push(`${LICENSE_INVENTORY} components reference ${hash}, which licenseTexts does not list`);
  for (const hash of declared.keys()) if (!named.has(hash)) errors.push(`${LICENSE_INVENTORY} licenseTexts lists unused text ${hash}`);
  for (const [hash, item] of declared) {
    const expected = [...(byHash.get(hash) ?? [])].sort();
    const actual = Array.isArray(item.components) ? [...item.components].sort() : [];
    if (JSON.stringify(expected) !== JSON.stringify(actual)) errors.push(`${LICENSE_INVENTORY} license text ${hash} component associations do not match component rows`);
  }
  return errors;
}

// ---------------------------------------------------------------------------
// THIRD_PARTY-NOTICES.md

function sectionBody(markdown, startMarker, endMarkers) {
  const start = markdown.indexOf(`${startMarker}\n`);
  if (start === -1) throw new Error(`Could not find "${startMarker}" in THIRD_PARTY-NOTICES.md`);
  const after = markdown.slice(start + startMarker.length);
  const ends = endMarkers.map((marker) => after.indexOf(`\n${marker}\n`)).filter((index) => index !== -1);
  return ends.length ? after.slice(0, Math.min(...ends)) : after;
}

/** One `### <ecosystem>` block of a `## <section>`, as Map<name, Set<license>>. */
export function parseNoticesGroup(markdown, section, heading) {
  const sectionText = sectionBody(markdown.replace(/\r\n/g, '\n'), section, [SHIPPED_SECTION, DEVELOPMENT_SECTION].filter((marker) => marker !== section));
  const deps = new Map();
  if (!sectionText.includes(`\n${heading}\n`)) return deps;
  const block = sectionBody(sectionText, heading, [RUST_HEADING, NPM_HEADING, ANDROID_HEADING].filter((marker) => marker !== heading));
  const lines = block.split('\n');
  for (let index = 0; index < lines.length; index += 1) {
    const match = /^#### (.+) \((\d+)\)$/.exec(lines[index]);
    if (!match) continue;
    let next = index + 1;
    while (next < lines.length && lines[next].trim() === '') next += 1;
    if (next >= lines.length) continue;
    for (const name of lines[next].split(',').map((value) => value.trim()).filter(Boolean)) {
      if (!deps.has(name)) deps.set(name, new Set());
      deps.get(name).add(match[1]);
    }
  }
  return deps;
}

export function diffDependencies(actual, declared) {
  const missing = [];
  const stale = [];
  const mismatched = [];
  for (const [name, licenses] of actual) {
    if (!declared.has(name)) { missing.push({ name, licenses: [...licenses] }); continue; }
    const declaredLicenses = declared.get(name);
    if (![...licenses].some((license) => declaredLicenses.has(license))) mismatched.push({ name, actual: [...licenses], declared: [...declaredLicenses] });
  }
  for (const [name, licenses] of declared) if (!actual.has(name)) stale.push({ name, licenses: [...licenses] });
  return { missing, stale, mismatched };
}

function renderGroup(heading, dependencies) {
  const byLicense = new Map();
  for (const [name, licenses] of dependencies) {
    for (const license of licenses) {
      if (!byLicense.has(license)) byLicense.set(license, []);
      byLicense.get(license).push(name);
    }
  }
  const lines = [heading];
  for (const license of [...byLicense.keys()].sort((left, right) => left.localeCompare(right))) {
    const names = byLicense.get(license).sort((left, right) => left.localeCompare(right));
    lines.push('', `#### ${license} (${names.length})`, '', names.join(', '));
  }
  return lines.join('\n');
}

const SHIPPED_INTRO = `These components are compiled into, or bundled with, the Windows app, the
device broker, or the Android app. The machine-readable inventory records each
one's exact version, and the source offer covers every one of them under a
copyleft license.`;

const DEVELOPMENT_INTRO = `These packages are locked in the same lockfiles but only build, test, lint or
deploy QC Remote: compilers and bundlers, test runners, image tooling, and the
packages only the hosted Cloudflare relay uses. None of them is distributed
with a QC Remote build, so they carry no source offer. They are listed so the
Rust/npm development dependency record is explicit. Android test and debug-only
configurations are outside this distributed-component inventory.`;

/** The generated part of THIRD_PARTY-NOTICES.md, below the hand-kept preamble. */
export function renderNotices(preamble, { shipped, development }) {
  return [
    preamble.trimEnd(),
    `${SHIPPED_SECTION}\n\n${SHIPPED_INTRO}`,
    renderGroup(RUST_HEADING, shipped.rust),
    renderGroup(NPM_HEADING, shipped.npm),
    renderGroup(ANDROID_HEADING, shipped.android),
    `${DEVELOPMENT_SECTION}\n\n${DEVELOPMENT_INTRO}`,
    renderGroup(RUST_HEADING, development.rust),
    renderGroup(NPM_HEADING, development.npm),
  ].join('\n\n') + '\n';
}

function noticesPreamble(markdown) {
  for (const marker of [SHIPPED_SECTION, '## Rust crates']) {
    const index = markdown.indexOf(`\n${marker}\n`);
    if (index !== -1) return markdown.slice(0, index + 1);
  }
  throw new Error(`Could not find "${SHIPPED_SECTION}" in THIRD_PARTY-NOTICES.md`);
}

const without = (all, shippedNames) => new Map([...all].filter(([name]) => !shippedNames.has(name)));

function reportDiff(label, { missing, stale, mismatched }) {
  if (missing.length === 0 && stale.length === 0 && mismatched.length === 0) return true;
  console.error(`\n${label}:`);
  for (const { name, licenses } of missing) console.error(`  MISSING from THIRD_PARTY-NOTICES.md: ${name} (${licenses.join(' / ')})`);
  for (const { name, licenses } of stale) console.error(`  STALE in THIRD_PARTY-NOTICES.md (not in this section of the locked graph): ${name} (${licenses.join(' / ')})`);
  for (const { name, actual, declared } of mismatched) console.error(`  LICENSE MISMATCH for ${name}: lockfiles say ${actual.join(' / ')}, notices say ${declared.join(' / ')}`);
  return false;
}

// ---------------------------------------------------------------------------

function main() {
  const read = (relative) => fs.readFileSync(path.join(repoRoot, relative), 'utf8');
  const noticesPath = path.join(repoRoot, 'THIRD_PARTY-NOTICES.md');
  const notices = fs.readFileSync(noticesPath, 'utf8').replace(/\r\n/g, '\n');
  const inventory = JSON.parse(read(LICENSE_INVENTORY));
  const lock = JSON.parse(read('package-lock.json'));

  const shippedCargo = new Map();
  for (const { manifest, target } of SHIPPED_CARGO_ROOTS) {
    const manifestPath = path.join(repoRoot, manifest);
    for (const [key, item] of shippedCargoPackages(cargoMetadata(manifestPath, target), manifestPath)) shippedCargo.set(key, item);
  }
  const shippedNpm = shippedNpmPackages(lock);
  const androidLocked = androidRuntimeLibraries(read(ANDROID_LOCKFILE));
  const androidLicenses = new Map();
  for (const component of inventory.components ?? []) {
    if (component.ecosystem === 'gradle' && androidLocked.has(componentKey(component))) addLicense(androidLicenses, component.name, component.selectedLicense ?? component.license);
  }

  const allCargo = allCargoPackages(ALL_CARGO_MANIFESTS.map((manifest) => ({ manifest, metadata: cargoMetadata(path.join(repoRoot, manifest)) })));
  const allNpm = allNpmPackages(lock);
  const shipped = { rust: byName(shippedCargo), npm: byName(shippedNpm), android: androidLicenses };
  const development = { rust: without(allCargo.deps, new Set(shipped.rust.keys())), npm: without(allNpm.deps, new Set(shipped.npm.keys())) };

  if (process.argv.includes('--write')) {
    fs.writeFileSync(noticesPath, renderNotices(noticesPreamble(notices), { shipped, development }), 'utf8');
    console.log('Regenerated THIRD_PARTY-NOTICES.md from the locked dependency graphs.');
    return;
  }

  let ok = true;
  try {
    ok = reportDiff('Shipped Rust crates', diffDependencies(shipped.rust, parseNoticesGroup(notices, SHIPPED_SECTION, RUST_HEADING))) && ok;
    ok = reportDiff('Shipped npm packages', diffDependencies(shipped.npm, parseNoticesGroup(notices, SHIPPED_SECTION, NPM_HEADING))) && ok;
    ok = reportDiff('Shipped Android libraries', diffDependencies(shipped.android, parseNoticesGroup(notices, SHIPPED_SECTION, ANDROID_HEADING))) && ok;
    ok = reportDiff('Development tooling: Rust crates', diffDependencies(development.rust, parseNoticesGroup(notices, DEVELOPMENT_SECTION, RUST_HEADING))) && ok;
    ok = reportDiff('Development tooling: npm packages', diffDependencies(development.npm, parseNoticesGroup(notices, DEVELOPMENT_SECTION, NPM_HEADING))) && ok;
  } catch (error) {
    ok = false;
    console.error(`\n${error instanceof Error ? error.message : String(error)}: it must separate shipped components from development tooling.`);
  }

  const recordErrors = [
    ...inventoryErrors(inventory, { cargo: shippedCargo, npm: shippedNpm, android: androidLocked }),
    ...sourceOfferErrors(inventory, read(SOURCE_OFFER)),
    ...licenseTextErrors(inventory, read(LICENSE_TEXTS)),
  ];
  if (recordErrors.length) {
    ok = false;
    console.error('\nLicense inventory, source offer and license texts:');
    for (const error of recordErrors) console.error(`  ${error}`);
  }

  for (const entry of [...allCargo.noLicense, ...allNpm.noLicense]) console.warn(`WARNING: no license metadata available for ${entry}; could not check it against the notices file.`);

  if (!ok) {
    console.error(
      '\nThe third-party legal record no longer matches Cargo.lock (root + apps/windows/src-tauri), package-lock.json and/or ' +
        `${ANDROID_LOCKFILE}. Regenerate the notices file (--write) and the inventory, texts and source offer, and commit the update.`,
    );
    process.exit(1);
  }
  console.log(
    `THIRD_PARTY-NOTICES.md, the license inventory, texts and source offer match Cargo.lock (root workspace + apps/windows/src-tauri), package-lock.json and ${ANDROID_LOCKFILE}.`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) main();
