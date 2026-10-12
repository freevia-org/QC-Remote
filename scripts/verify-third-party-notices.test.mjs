// Fixture tests for scripts/verify-third-party-notices.mjs,
// and the whitespace exemption for the licence-text bundles it verifies.
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  DEVELOPMENT_SECTION,
  NPM_HEADING,
  RUST_HEADING,
  SHIPPED_SECTION,
  SHIPPED_CARGO_ROOTS,
  androidRuntimeLibraries,
  diffDependencies,
  inventoryErrors,
  licenseTextErrors,
  normalizeLicense,
  parseNoticesGroup,
  renderNotices,
  shippedCargoPackages,
  shippedNpmPackages,
  sourceOfferErrors,
  sourceOfferRows,
} from './verify-third-party-notices.mjs';

test('the shipped broker inventory includes both native dependency graphs', () => {
  assert.deepEqual(
    SHIPPED_CARGO_ROOTS.filter(({ manifest }) => manifest === 'services/device-broker/Cargo.toml')
      .map(({ target }) => target).sort(),
    ['x86_64-pc-windows-msvc', 'x86_64-unknown-linux-gnu'],
  );
});

test('the shipped desktop inventory enables the resident host on both platforms', () => {
  assert.deepEqual(
    SHIPPED_CARGO_ROOTS.filter(({ manifest }) => manifest === 'apps/windows/src-tauri/Cargo.toml')
      .map(({ target, features }) => [target, features]).sort(),
    [['x86_64-pc-windows-msvc', 'tray-sidecar'], ['x86_64-unknown-linux-gnu', 'tray-sidecar']],
  );
});

test('license expressions normalize to one SPDX spelling per choice', () => {
  for (const spelling of ['MIT OR Apache-2.0', 'Apache-2.0 OR MIT', 'MIT/Apache-2.0', 'Apache-2.0/MIT', 'Apache-2.0 / MIT']) {
    assert.equal(normalizeLicense(spelling), 'Apache-2.0 OR MIT', spelling);
  }
  assert.equal(normalizeLicense('Unlicense/MIT'), 'MIT OR Unlicense');
  assert.equal(normalizeLicense('The Apache Software License, Version 2.0'), 'Apache-2.0');
  assert.equal(normalizeLicense('The Apache License, Version 2.0'), 'Apache-2.0');
  assert.equal(normalizeLicense('Apache License, Version 2.0'), 'Apache-2.0');
  assert.equal(normalizeLicense('Apache 2.0'), 'Apache-2.0');
  assert.equal(normalizeLicense('The MIT License'), 'MIT');
  assert.equal(normalizeLicense('GNU General Public License, version 2 (GPL2), with the classpath exception OR The MIT License'), 'GPL-2.0-only WITH Classpath-exception-2.0 OR MIT');
  assert.equal(normalizeLicense('(MIT OR Apache-2.0) AND Unicode-3.0'), '(Apache-2.0 OR MIT) AND Unicode-3.0');
  assert.equal(normalizeLicense('MIT AND BSD-3-Clause'), 'MIT AND BSD-3-Clause');
});

const lock = {
  packages: {
    '': { workspaces: ['apps/*', 'packages/*'] },
    'apps/windows': { name: '@qc/windows', dependencies: { react: '^19', '@qc/ui': '0.1.0' }, devDependencies: { vite: '^8' } },
    'apps/android': { name: '@qc/android', dependencies: { '@capacitor/core': '^8' } },
    'packages/ui': { name: '@qc/ui', dependencies: { qrcode: '^1' }, peerDependencies: { react: '>=19' } },
    'node_modules/@qc/ui': { resolved: 'packages/ui', link: true },
    'node_modules/react': { version: '19.3.0', license: 'MIT' },
    'node_modules/qrcode': { version: '1.5.4', license: 'MIT', dependencies: { pngjs: '^5' } },
    'node_modules/pngjs': { version: '5.0.0', license: 'MIT' },
    'node_modules/qrcode/node_modules/pngjs': { version: '6.0.0', license: 'MIT' },
    'node_modules/@capacitor/core': { version: '8.5.2', license: 'MIT' },
    'node_modules/vite': { version: '8.2.2', license: 'MIT', dev: true, dependencies: { sharp: '*' } },
    'node_modules/sharp': { version: '0.35.5', license: 'Apache-2.0', dev: true },
  },
};

test('the shipped npm set is what the app workspaces reach at runtime, nested copies resolved like Node', () => {
  const shipped = shippedNpmPackages(lock);
  assert.deepEqual([...shipped.keys()].sort(), ['@capacitor/core@8.5.2', 'pngjs@6.0.0', 'qrcode@1.5.4', 'react@19.3.0']);
  assert.ok(!shipped.has('vite@8.2.2') && !shipped.has('sharp@0.35.5'), 'build tooling is not shipped');
});

test('the shipped Cargo set follows normal and build edges from the root, never dev edges', () => {
  const root = path.resolve('fixture/app/Cargo.toml');
  const metadata = {
    packages: [
      { id: 'app', name: 'app', version: '0.1.0', source: null, manifest_path: root },
      { id: 'serde', name: 'serde', version: '1.0.0', source: 'registry', license: 'MIT OR Apache-2.0', manifest_path: '/r/serde/Cargo.toml' },
      { id: 'cc', name: 'cc', version: '1.4.7', source: 'registry', license: 'MIT OR Apache-2.0', manifest_path: '/r/cc/Cargo.toml' },
      { id: 'proptest', name: 'proptest', version: '1.0.0', source: 'registry', license: 'MIT OR Apache-2.0', manifest_path: '/r/proptest/Cargo.toml' },
    ],
    resolve: {
      nodes: [
        { id: 'app', deps: [
          { pkg: 'serde', dep_kinds: [{ kind: null }] },
          { pkg: 'cc', dep_kinds: [{ kind: 'build' }] },
          { pkg: 'proptest', dep_kinds: [{ kind: 'dev' }] },
        ] },
      ],
    },
  };
  assert.deepEqual([...shippedCargoPackages(metadata, root).keys()].sort(), ['cc@1.4.7', 'serde@1.0.0']);
});

const shippedFixture = () => ({
  cargo: new Map([['syn@3.0.6', { name: 'syn', version: '3.0.6', license: 'MIT OR Apache-2.0' }]]),
  npm: new Map([['react@19.3.0', { name: 'react', version: '19.3.0', license: 'MIT' }]]),
  android: androidRuntimeLibraries('androidx.core:core:1.13.0=debugRuntimeClasspath,releaseRuntimeClasspath\nfoo:bar:1=debugCompileClasspath\n'),
});
const component = (ecosystem, name, version, license) => ({ ecosystem, name, version, license });

test('Android shipping inventory requires release runtime dependencies and excludes test configurations', () => {
  const android = androidRuntimeLibraries([
    'example:shared:1=debugRuntimeClasspath,releaseRuntimeClasspath',
    'example:release:2=releaseRuntimeClasspath',
    'example:debug:3=debugRuntimeClasspath',
    'junit:junit:4.13.2=debugUnitTestCompileClasspath,debugUnitTestRuntimeClasspath',
    'example:instrumentation:4=debugAndroidTestRuntimeClasspath',
    'example:release-test:5=releaseUnitTestRuntimeClasspath,releaseAndroidTestRuntimeClasspath',
  ].join('\n'));
  assert.deepEqual([...android].sort(), ['example:release@2', 'example:shared@1']);
  const graph = { cargo: new Map(), npm: new Map(), android };
  const inventory = { components: [
    component('gradle', 'example:shared', '1', 'MIT'),
    component('gradle', 'example:release', '2', 'MIT'),
  ] };
  assert.deepEqual(inventoryErrors(inventory, graph), []);
  assert.ok(inventoryErrors({ components: inventory.components.slice(0, 1) }, graph)
    .some((error) => error.includes('does not record gradle:example:release@2')));
});

test('the inventory must record exactly the shipped versions; a name-only match is drift', () => {
  const current = { components: [component('cargo', 'syn', '3.0.6', 'MIT OR Apache-2.0'), component('npm', 'react', '19.3.0', 'MIT'), component('gradle', 'androidx.core:core', '1.13.0', 'Apache-2.0')] };
  assert.deepEqual(inventoryErrors(current, shippedFixture()), []);
  // The shape of the drift this check exists to catch: the right names at older versions.
  const stale = { components: [component('cargo', 'syn', '3.0.4', 'MIT OR Apache-2.0'), component('npm', 'react', '19.2.8', 'MIT'), component('gradle', 'androidx.core:core', '1.13.0', 'Apache-2.0')] };
  const errors = inventoryErrors(stale, shippedFixture());
  assert.ok(errors.some((error) => error.includes('does not record cargo:syn@3.0.6')));
  assert.ok(errors.some((error) => error.includes('records cargo:syn@3.0.4')));
  assert.ok(errors.some((error) => error.includes('does not record npm:react@19.3.0')));
  assert.ok(errors.some((error) => error.includes('records npm:react@19.2.8')));
  const relicensed = { components: [...current.components.slice(0, 1), component('npm', 'react', '19.3.0', 'Apache-2.0'), current.components[2]] };
  assert.ok(inventoryErrors(relicensed, shippedFixture()).some((error) => error.includes('as Apache-2.0, but it declares MIT')));
  const androidDrift = { components: [...current.components.slice(0, 2), component('gradle', 'androidx.core:core', '1.12.0', 'Apache-2.0')] };
  assert.equal(inventoryErrors(androidDrift, shippedFixture()).filter((error) => error.includes('gradle:androidx.core:core')).length, 2);
});

const archive = (name, version) => `https://crates.io/api/v1/crates/${name}/${version}/download`;
const offerInventory = {
  components: [
    { ...component('cargo', 'cssparser', '0.36.0', 'MPL-2.0'), sourceArchiveUrl: archive('cssparser', '0.36.0') },
    component('cargo', 'serde', '1.0.0', 'MIT OR Apache-2.0'),
    { ...component('gradle', 'org.checkerframework:checker-compat-qual', '2.5.5', 'GNU General Public License, version 2 (GPL2), with the classpath exception OR The MIT License'), selectedLicense: 'MIT' },
  ],
  sourceAvailabilityNotice: [{ component: 'cargo:cssparser@0.36.0', sourceArchiveUrl: archive('cssparser', '0.36.0') }],
};
const offer = (rows) => ['# Source', '', 'Selection rule: every shipped copyleft component; development tooling is not distributed.', '', '| Component | Exact source archive | Upstream project |', '| --- | --- | --- |', ...rows, ''].join('\n');

test('the source offer lists exactly the shipped copyleft components at their exact versions and states its rule', () => {
  const good = offer([`| cargo:cssparser@0.36.0 | ${archive('cssparser', '0.36.0')} | https://github.com/servo/rust-cssparser |`]);
  assert.deepEqual(sourceOfferRows(good).map((row) => row.component), ['cargo:cssparser@0.36.0']);
  assert.deepEqual(sourceOfferErrors(offerInventory, good), []);
  assert.ok(sourceOfferErrors(offerInventory, offer([])).some((error) => error.includes('lists nothing')));
  const wrongVersion = offer([`| cargo:cssparser@0.36.0 | ${archive('cssparser', '0.35.0')} | x |`]);
  assert.ok(sourceOfferErrors(offerInventory, wrongVersion).some((error) => error.includes('not that exact version')));
  const extra = offer([`| cargo:cssparser@0.36.0 | ${archive('cssparser', '0.36.0')} | x |`, `| npm:sharp@0.35.5 | x | y |`]);
  assert.ok(sourceOfferErrors(offerInventory, extra).some((error) => error.includes('npm:sharp@0.35.5, which legal/THIRD_PARTY-LICENSE-INVENTORY.json does not record')));
  assert.ok(sourceOfferErrors(offerInventory, good.replace('Selection rule', 'Rule')).some((error) => error.includes('selection rule')));
});

test('license-text digests verify bundled bodies and reject changed or missing text', () => {
  const body = 'Permission to use this example.\nKeep this notice.';
  const hash = createHash('sha256').update(body).digest('hex');
  const bundle = `${'='.repeat(80)}\nSHA-256: ${hash}\nSource file names: LICENSE\nApplies to: npm:example@1\n${'-'.repeat(80)}\n${body}\n`;
  const inventory = {
    components: [{ ecosystem: 'npm', name: 'example', version: '1', licenseTextHashes: [hash] }],
    licenseTexts: [{ sha256: hash, components: ['npm:example@1'] }],
    missingLocalLicenseText: [],
  };
  assert.deepEqual(licenseTextErrors(inventory, bundle), []);
  assert.deepEqual(licenseTextErrors(inventory, bundle.replace(/\n/g, '\r\n')), []);
  assert.ok(licenseTextErrors(inventory, bundle.replace('Permission', 'Forbidden')).some((error) => error.includes('body hashes to')));
  assert.ok(licenseTextErrors(inventory, `SHA-256: ${hash}\n`).some((error) => error.includes('no body record')));
  assert.ok(licenseTextErrors(inventory, '').some((error) => error.includes('does not contain')));
  assert.ok(licenseTextErrors({ components: [], licenseTexts: [] }, bundle).some((error) => error.includes(`holds text ${hash}`)));
  assert.ok(licenseTextErrors(inventory, bundle + bundle).some((error) => error.includes('repeats license text')));
  const reusedHash = licenseTextErrors(inventory, bundle + bundle.replace('Permission', 'Forbidden'));
  assert.ok(reusedHash.some((error) => error.includes('repeats license text')));
  assert.ok(reusedHash.some((error) => error.includes('body hashes to')));
  assert.ok(licenseTextErrors(inventory, bundle.replace('-'.repeat(80), 'missing divider')).some((error) => error.includes('malformed')));

  const missingButReported = { components: [{ ecosystem: 'cargo', name: 'missing', version: '1', licenseTextHashes: [] }], licenseTexts: [], missingLocalLicenseText: ['cargo:missing@1'] };
  assert.ok(licenseTextErrors(missingButReported, '').some((error) => error.includes('has no associated license text')));

  const partial = structuredClone(inventory);
  partial.components.push({ ecosystem: 'cargo', name: 'missing', version: '2', licenseTextHashes: [] });
  partial.missingLocalLicenseText = ['cargo:missing@2'];
  assert.ok(licenseTextErrors(partial, bundle).some((error) => error.includes('cargo:missing@2 has no associated license text')));

  const staleSummary = structuredClone(inventory);
  staleSummary.missingLocalLicenseText = ['cargo:stale@2'];
  assert.ok(licenseTextErrors(staleSummary, bundle).some((error) => error.includes('missingLocalLicenseText')));

  const staleReverseMap = structuredClone(inventory);
  staleReverseMap.licenseTexts[0].components = ['npm:other@1'];
  assert.ok(licenseTextErrors(staleReverseMap, bundle).some((error) => error.includes('component associations')));

  const missingReference = structuredClone(inventory);
  missingReference.components[0].licenseTextHashes = ['b'.repeat(64)];
  assert.ok(licenseTextErrors(missingReference, bundle).some((error) => error.includes('components reference')));

  const orphanBody = 'A second, unreferenced license body.';
  const orphanHash = createHash('sha256').update(orphanBody).digest('hex');
  const orphanBundle = `${'='.repeat(80)}\nSHA-256: ${orphanHash}\nSource file names: LICENSE-OTHER\nApplies to: cargo:other@1\n${'-'.repeat(80)}\n${orphanBody}\n`;
  const orphanInventory = structuredClone(inventory);
  orphanInventory.licenseTexts.push({ sha256: orphanHash, components: ['cargo:other@1'] });
  assert.ok(licenseTextErrors(orphanInventory, bundle + orphanBundle).some((error) => error.includes('unused text')));
});

test('the notices file separates shipped components from development tooling and parses back', () => {
  const rendered = renderNotices('# Third-party notices\n\nPreamble.\n', {
    shipped: { rust: new Map([['syn', new Set(['Apache-2.0 OR MIT'])]]), npm: new Map([['react', new Set(['MIT'])]]), android: new Map() },
    development: { rust: new Map([['proptest', new Set(['Apache-2.0 OR MIT'])]]), npm: new Map([['sharp', new Set(['Apache-2.0'])], ['vite', new Set(['MIT'])]]) },
  });
  assert.ok(rendered.indexOf(SHIPPED_SECTION) < rendered.indexOf(DEVELOPMENT_SECTION));
  assert.deepEqual([...parseNoticesGroup(rendered, SHIPPED_SECTION, NPM_HEADING).keys()], ['react']);
  assert.deepEqual([...parseNoticesGroup(rendered, DEVELOPMENT_SECTION, NPM_HEADING).keys()].sort(), ['sharp', 'vite']);
  assert.deepEqual([...parseNoticesGroup(rendered, SHIPPED_SECTION, RUST_HEADING).keys()], ['syn']);
  // Listing development tooling as shipped is reported.
  const tooling = diffDependencies(new Map([['react', new Set(['MIT'])]]), parseNoticesGroup(rendered.replace('react', 'react, vite'), SHIPPED_SECTION, NPM_HEADING));
  assert.deepEqual(tooling.stale.map((entry) => entry.name), ['vite']);
  assert.throws(() => parseNoticesGroup('# Notices\n\n## Rust crates\n', SHIPPED_SECTION, RUST_HEADING), /Shipped components/);
});

// The two licence-text bundles carry upstream licence files byte for byte,
// trailing blanks included, and each record's SHA-256 pins its source, so
// .gitattributes exempts exactly those two paths from the blank-at-eol check
// that `git diff --check` applies. Every
// other file, the neighbouring legal documents and a same-named file elsewhere
// included, stays checked.
test('only the two licence-text bundles are exempt from the trailing-blank whitespace check', () => {
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const root = mkdtempSync(path.join(os.tmpdir(), 'qc-licence-whitespace-'));
  const git = (...args) => execFileSync('git', ['-C', root, '-c', 'core.autocrlf=false', '-c', 'core.whitespace=blank-at-eol,space-before-tab,blank-at-eof', ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  const exempt = ['legal/THIRD_PARTY-LICENSE-TEXTS.txt', 'skills/qc-device-knowledge/legal/THIRD_PARTY-LICENSE-TEXTS.txt'];
  const checked = [
    'src/main.rs',
    'legal/THIRD_PARTY-SOURCE-OFFER.md',
    'THIRD_PARTY-NOTICES.md',
    'other/legal/THIRD_PARTY-LICENSE-TEXTS.txt',
    'skills/other-skill/legal/THIRD_PARTY-LICENSE-TEXTS.txt',
  ];
  try {
    git('init', '-q');
    git('config', 'user.name', 'test');
    git('config', 'user.email', 'test@example.invalid');
    writeFileSync(path.join(root, '.gitattributes'), readFileSync(path.join(repositoryRoot, '.gitattributes')));
    for (const file of [...exempt, ...checked]) {
      mkdirSync(path.join(root, path.dirname(file)), { recursive: true });
      writeFileSync(path.join(root, file), 'first line\n');
    }
    git('add', '-A');
    git('commit', '-q', '-m', 'base');
    git('branch', 'base');
    for (const file of [...exempt, ...checked]) writeFileSync(path.join(root, file), 'first line\n(C) 2024 Upstream Authors \n');
    git('commit', '-q', '-a', '-m', 'trailing blanks');
    let report = '';
    try {
      git('diff', '--check', 'base...HEAD');
    } catch (error) {
      report = String(error.stdout);
    }
    const reported = new Set([...report.matchAll(/^([^\s:+][^:\n]*):\d+: trailing whitespace\./gm)].map((match) => match[1]));
    assert.deepEqual([...reported].sort(), [...checked].sort(), report);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
