import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const vendorRoot = path.join(root, 'apps/windows/src-tauri/vendor');
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

function readCrateArchive(archive) {
  const tar = gunzipSync(archive);
  const files = new Map();
  let offset = 0;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const field = (start, length) => header.subarray(start, start + length).toString('utf8').replace(/\0.*$/s, '');
    const name = field(0, 100);
    const prefix = field(345, 155);
    const archivePath = prefix ? `${prefix}/${name}` : name;
    const type = header[156];
    assert.ok(type === 0 || type === 48, `unexpected non-file archive entry: ${archivePath}`);
    const size = Number.parseInt(field(124, 12).trim() || '0', 8);
    assert.ok(Number.isSafeInteger(size) && size >= 0, `invalid tar size for ${archivePath}`);
    const dataStart = offset + 512;
    files.set(archivePath, tar.subarray(dataStart, dataStart + size));
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return files;
}

function treeFiles(directory, relative = '') {
  const files = [];
  for (const entry of readdirSync(path.join(directory, relative), { withFileTypes: true })) {
    const child = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isDirectory()) files.push(...treeFiles(directory, child));
    else if (entry.isFile()) files.push(child);
    else assert.fail(`unexpected non-file vendor entry: ${child}`);
  }
  return files;
}

const crates = [
  { package: 'ksni', version: '0.3.7', directory: 'ksni-0.3.7' },
  { package: 'tauri-plugin-single-instance', version: '2.5.5', directory: 'tauri-plugin-single-instance-2.5.5' },
];

for (const item of crates) {
  test(`${item.package} vendor tree is pinned to its published archive and reviewed zbus 6 patch`, () => {
    const crate = path.join(vendorRoot, item.directory);
    const provenance = JSON.parse(readFileSync(path.join(crate, 'PATCH-PROVENANCE.json'), 'utf8'));
    assert.equal(provenance.package, item.package);
    assert.equal(provenance.version, item.version);
    assert.equal(provenance.publishedArchive, `https://crates.io/api/v1/crates/${item.package}/${item.version}/download`);
    assert.equal(provenance.sourceLockChecksum, provenance.publishedArchiveSha256);
    assert.match(provenance.publishedArchiveSha256, /^[a-f0-9]{64}$/);

    const archivePath = path.join(crate, `${item.package}-${item.version}.crate`);
    const archive = readFileSync(archivePath);
    assert.equal(sha256(archive), provenance.publishedArchiveSha256);
    const published = readCrateArchive(archive);
    const prefix = `${item.package}-${item.version}/`;
    assert.ok([...published.keys()].every((name) => name.startsWith(prefix)), 'archive entries remain under the crate root');
    const archiveNames = [...published.keys()].map((name) => name.slice(prefix.length)).sort();
    const patches = new Map(provenance.patches.map((patch) => [patch.path, patch]));
    assert.equal(patches.size, provenance.patches.length, 'patch file list has no duplicate paths');
    const expectedNames = [...archiveNames, 'PATCH-PROVENANCE.json', 'PATCH-PROVENANCE.md', `${item.package}-${item.version}.crate`].sort();
    assert.deepEqual(treeFiles(crate).sort(), expectedNames, 'vendor tree contains only published files and pinned provenance');

    const changed = [];
    for (const [archiveName, upstreamBytes] of published) {
      const relative = archiveName.slice(prefix.length);
      const vendoredBytes = readFileSync(path.join(crate, ...relative.split('/')));
      if (Buffer.compare(upstreamBytes, vendoredBytes) === 0) continue;
      changed.push(relative);
      const patch = patches.get(relative);
      assert.ok(patch, `unreviewed vendor source change: ${relative}`);
      assert.equal(sha256(upstreamBytes), patch.upstreamSha256, `unexpected archive preimage: ${relative}`);
      assert.equal(sha256(vendoredBytes), patch.patchedSha256, `vendor patch changed without provenance update: ${relative}`);
      assert.ok(patch.reason.length > 20, `patch rationale is missing: ${relative}`);
    }
    assert.deepEqual(changed.sort(), [...patches.keys()].sort(), 'every and only declared patch file differs from the crate archive');
    assert.equal(provenance.patches.length, patches.size);
  });
}

test('zbus 6 runtime and Linux session-bus regressions stay in the shipped locked app graph', () => {
  const appManifestPath = path.join(root, 'apps/windows/src-tauri/Cargo.toml');
  const appManifest = readFileSync(appManifestPath, 'utf8');
  assert.match(appManifest, /^ksni = \{ path = "vendor\/ksni-0\.3\.7" \}$/m);
  assert.match(appManifest, /^tauri-plugin-single-instance = \{ path = "vendor\/tauri-plugin-single-instance-2\.5\.5" \}$/m);
  assert.match(appManifest, /\[target\.'cfg\(target_os = "linux"\)'\.dev-dependencies\][\s\S]*?zbus = \{ version = "=6\.5\.0"/);
  assert.match(appManifest, /tauri = \{ version = "2", default-features = false, features = \["test"\] \}/);

  const brokerManifest = readFileSync(path.join(root, 'services/device-broker/Cargo.toml'), 'utf8');
  assert.match(brokerManifest, /^zbus = \{ version = "=6\.5\.0".*"proxy"\]/m);
  const workspaceManifest = readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
  assert.match(workspaceManifest, /^ksni = \{ path = "apps\/windows\/src-tauri\/vendor\/ksni-0\.3\.7" \}$/m);
  const broker = readFileSync(path.join(root, 'services/device-broker/src/tray/linux.rs'), 'utf8');
  assert.doesNotMatch(broker, /zbus::blocking/);
  assert.match(broker, /zbus::block_on\(async/);
  assert.match(broker, /IsStatusNotifierHostRegistered/);

  const plugin = path.join(vendorRoot, 'tauri-plugin-single-instance-2.5.5');
  const pluginManifest = readFileSync(path.join(plugin, 'Cargo.toml'), 'utf8');
  assert.match(pluginManifest, /\[target\.'cfg\(target_os = "linux"\)'\.dependencies\.zbus\][\s\S]*?version = "6\.5"[\s\S]*?features = \["default-rt", "service"\]/);
  const pluginLinux = readFileSync(path.join(plugin, 'src/platform_impl/linux.rs'), 'utf8');
  assert.doesNotMatch(pluginLinux, /zbus::blocking/);
  assert.match(pluginLinux, /\.release_name\(&self\.name\)/);
  assert.match(pluginLinux, /cleanup_before_exit\(\)/);
  assert.match(pluginLinux, /\.call_method\(/);

  const lock = readFileSync(path.join(root, 'apps/windows/src-tauri/Cargo.lock'), 'utf8');
  assert.match(lock, /name = "zbus"\nversion = "6\.5\.0"/);
  for (const removed of ['zbus_names', 'zvariant', 'zvariant_derive']) {
    assert.doesNotMatch(lock, new RegExp(`name = "${removed}"`), `${removed} must not remain as a standalone package`);
  }
  const workspaceLock = readFileSync(path.join(root, 'Cargo.lock'), 'utf8');
  assert.match(workspaceLock, /name = "zbus"\nversion = "6\.5\.0"/);
  assert.doesNotMatch(workspaceLock, /name = "zbus"\nversion = "5\./, 'the broker workspace must not resolve a zbus 5 runtime');

  const linuxTests = ['dbus_errors.rs', 'instance_lock.rs'];
  for (const file of linuxTests) {
    const provenance = JSON.parse(readFileSync(path.join(plugin, 'PATCH-PROVENANCE.json'), 'utf8'));
    const record = provenance.consumerTests.find((entry) => entry.path.endsWith(`/${file}`));
    assert.ok(record, `missing upstream provenance for ${file}`);
    const actual = readFileSync(path.join(root, record.path));
    assert.equal(sha256(actual), record.patchedSha256, `${file} differs from its reviewed upstream test adaptation`);
  }
  const errorsTest = readFileSync(path.join(root, 'apps/windows/src-tauri/tests/dbus_errors.rs'), 'utf8');
  assert.match(errorsTest, /^#!\[cfg\(target_os = "linux"\)\]/m);
  const instanceTest = readFileSync(path.join(root, 'apps/windows/src-tauri/tests/instance_lock.rs'), 'utf8');
  for (const behavior of ['destroy_releases_the_lock', 'cleanup_before_exit_releases_the_lock', 'lock_is_held_during_the_exit_event', 'other_runtimes']) {
    assert.ok(instanceTest.includes(behavior), `missing single-instance behavior case: ${behavior}`);
  }

  const workflow = readFileSync(path.join(root, '.github/workflows/linux-broker.yml'), 'utf8');
  assert.match(workflow, /dbus-run-session -- cargo \+stable test --locked --features tray-sidecar,custom-protocol \\\s+--test instance_lock --test dbus_errors -- --include-ignored/);
  assert.match(workflow, /dbus-run-session -- python3 -m unittest scripts\.test_linux_tray_startup -v/);
});
