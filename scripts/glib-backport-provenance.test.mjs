import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const crate = path.join(root, 'apps/windows/src-tauri/vendor/glib-0.18.5');
const provenance = JSON.parse(readFileSync(path.join(crate, 'PATCH-PROVENANCE.json'), 'utf8'));

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
    assert.ok(type === 0 || type === 48, `unexpected non-file entry in published crate archive: ${archivePath}`);
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
    else assert.fail(`unexpected non-file in vendored glib tree: ${child}`);
  }
  return files;
}

test('vendored glib backport records the published source and exact upstream fix', () => {
  assert.equal(provenance.package, 'glib');
  assert.equal(provenance.version, '0.18.5');
  assert.equal(provenance.publishedArchive, 'https://crates.io/api/v1/crates/glib/0.18.5/download');
  assert.equal(provenance.publishedArchiveSha256, '233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5');
  assert.equal(provenance.upstreamPatchCommit, '40b7211');
  assert.equal(provenance.upstreamPatchUrl, 'https://github.com/gtk-rs/gtk-rs-core/commit/40b7211');
  assert.match(provenance.publishedArchiveSha256, /^[a-f0-9]{64}$/);
  assert.deepEqual(provenance.patchScope, [
    'src/variant_iter.rs: make the GVariant string output pointer mutable, as in upstream commit 40b7211',
  ]);

  const manifest = readFileSync(path.join(crate, 'Cargo.toml'), 'utf8');
  assert.match(manifest, /^name = "glib"$/m);
  assert.match(manifest, /^version = "0\.18\.5"$/m);
  assert.match(manifest, /^license = "MIT"$/m);
  assert.match(manifest, /^repository = "https:\/\/github\.com\/gtk-rs\/gtk-rs-core"$/m);

  const variantIter = readFileSync(path.join(crate, 'src/variant_iter.rs'));
  const digest = sha256(variantIter);
  assert.equal(digest, provenance.patchedVariantIterSha256);
  const source = variantIter.toString('utf8');
  assert.match(source, /let mut p: \*mut libc::c_char = std::ptr::null_mut\(\);/);
  assert.match(source, /\n\s*&mut p,/);
  assert.doesNotMatch(source, /variant_str_iter_pointer_output_is_valid_across_optimized_iterator_paths/);

  const appManifest = readFileSync(path.join(root, 'apps/windows/src-tauri/Cargo.toml'), 'utf8');
  assert.match(appManifest, /\[target\.'cfg\(target_os = "linux"\)'\.dev-dependencies\][\s\S]*?glib = "=0\.18\.5"/);
  const regression = readFileSync(path.join(root, 'apps/windows/src-tauri/tests/glib_variant_iter_backport.rs'), 'utf8');
  assert.match(regression, /^#!\[cfg\(target_os = "linux"\)\]/);
  for (const operation of ['iter.next()', 'iter.nth(1)', 'iter.next_back()', 'iter.nth_back(1)', 'iter.last()']) assert.ok(regression.includes(operation), `optimized iterator regression must exercise ${operation}`);
  assert.match(regression, /assert_eq!\(iter\.next\(\), None\)/);
  const workflow = readFileSync(path.join(root, '.github/workflows/linux-broker.yml'), 'utf8');
  assert.match(workflow, /cargo \+stable test --release --locked --features tray-sidecar,custom-protocol --test glib_variant_iter_backport/);

  const archivePath = path.join(crate, 'glib-0.18.5.crate');
  const archive = readFileSync(archivePath);
  assert.equal(sha256(archive), provenance.publishedArchiveSha256, 'checked-in crate archive must match the published lock checksum');
  const publishedFiles = readCrateArchive(archive);
  const prefix = 'glib-0.18.5/';
  assert.ok([...publishedFiles.keys()].every((name) => name.startsWith(prefix)), 'archive entries must stay under the crate root');
  const expectedNames = [...publishedFiles.keys()].map((name) => name.slice(prefix.length)).sort();
  const extras = [
    'PATCH-PROVENANCE.json',
    'PATCH-PROVENANCE.md',
    'glib-0.18.5.crate',
  ];
  assert.deepEqual(treeFiles(crate).sort(), [...expectedNames, ...extras].sort(), 'vendor tree may contain only published files plus documented provenance');

  for (const [archiveName, bytes] of publishedFiles) {
    const relative = archiveName.slice(prefix.length);
    const vendored = readFileSync(path.join(crate, relative));
    if (relative === 'src/variant_iter.rs') {
      assert.equal(sha256(bytes), provenance.upstreamVariantIterSha256, 'upstream preimage hash must match the crate archive');
      const upstream = bytes.toString('utf8');
      const immutableBinding = 'let p: *mut libc::c_char = std::ptr::null_mut();';
      const immutableOutput = '                &p,';
      assert.equal(upstream.split(immutableBinding).length - 1, 1, 'upstream preimage must contain one expected pointer binding');
      assert.equal(upstream.split(immutableOutput).length - 1, 1, 'upstream preimage must contain one expected output argument');
      const expected = upstream
        .replace(immutableBinding, 'let mut p: *mut libc::c_char = std::ptr::null_mut();')
        .replace(immutableOutput, '                &mut p,');
      assert.deepEqual(vendored, Buffer.from(expected), 'vendored iterator source must equal the published file plus only upstream 40b7211');
      continue;
    }
    assert.deepEqual(vendored, bytes, `published crate file changed unexpectedly: ${relative}`);
  }
});
