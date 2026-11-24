import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

function archiveFiles(archive) {
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
    assert.ok(Number.isSafeInteger(size) && size >= 0, `invalid archive size: ${archivePath}`);
    const dataStart = offset + 512;
    files.set(archivePath, tar.subarray(dataStart, dataStart + size));
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return files;
}

function filesUnder(directory, relative = '') {
  const result = [];
  for (const entry of readdirSync(path.join(directory, relative), { withFileTypes: true })) {
    const child = relative ? `${relative}/${entry.name}` : entry.name;
    if (entry.isDirectory()) result.push(...filesUnder(directory, child));
    else if (entry.isFile()) result.push(child);
    else assert.fail(`unexpected non-file in vendored crate: ${child}`);
  }
  return result;
}

function replaceOnce(source, before, after, label) {
  assert.equal(source.split(before).length - 1, 1, `${label} must occur exactly once in published source`);
  return source.replace(before, after);
}

const crates = [
  {
    package: 'glib-macros', version: '0.18.5', archiveSha256: '0bb0228f477c0900c880fd78c8759b95c7636dbd7842707f49e132378aa2acdc',
    oldDependency: '[dependencies.proc-macro-error]\nversion = "1.0"\n',
    newDependency: '[dependencies.proc-macro-error3]\nversion = "3.1.1"\ndefault-features = false\nfeatures = ["syn2-error"]\n',
  },
  {
    package: 'gtk3-macros', version: '0.18.2', archiveSha256: '52ff3c5b21f14f0736fed6dcfc0bfb4225ebf5725f3c0209edeec181e4d73e9d',
    oldDependency: '[dependencies.proc-macro-error]\nversion = "1.0"\n',
    newDependency: '[dependencies.proc-macro-error3]\nversion = "3.1.1"\ndefault-features = false\nfeatures = ["syn2-error"]\n',
  },
];

test('vendored GTK macro crates differ from published archives only by the proc-macro-error3 migration', () => {
  for (const crate of crates) {
    const directory = path.join(root, 'apps/windows/src-tauri/vendor', `${crate.package}-${crate.version}`);
    const provenance = JSON.parse(readFileSync(path.join(directory, 'PATCH-PROVENANCE.json'), 'utf8'));
    assert.equal(provenance.package, crate.package);
    assert.equal(provenance.version, crate.version);
    assert.equal(provenance.publishedArchiveSha256, crate.archiveSha256);
    assert.equal(provenance.replacementPackage, 'proc-macro-error3');
    assert.equal(provenance.replacementVersion, '3.1.1');
    assert.deepEqual(provenance.patchScope, [
      'Cargo.toml and Cargo.toml.orig: replace proc-macro-error 1.x with proc-macro-error3 3.1.1 using syn2-error',
      'src/lib.rs: alias proc_macro_error3 as proc_macro_error for existing imports',
    ]);
    assert.deepEqual(provenance.packagingMetadataScope, [
      'Cargo.toml: add package.publish = false',
      'Cargo.toml.orig: add package.publish = false',
    ]);
    assert.deepEqual(provenance.formattingNormalizationScope, crate.package === 'glib-macros' ? [
      'LICENSE: remove the extra blank line at end of file',
      'src/lib.rs: remove trailing spaces from one rustdoc blank line',
    ] : ['LICENSE: remove the extra blank line at end of file']);

    const archivePath = path.join(directory, `${crate.package}-${crate.version}.crate`);
    const archive = readFileSync(archivePath);
    assert.equal(sha256(archive), crate.archiveSha256, 'checked-in crate archive must match the original registry checksum');
    const prefix = `${crate.package}-${crate.version}/`;
    const published = archiveFiles(archive);
    assert.ok([...published.keys()].every((name) => name.startsWith(prefix)), 'published files must remain inside their crate root');

    const extras = ['PATCH-PROVENANCE.json', 'PATCH-PROVENANCE.md', `${crate.package}-${crate.version}.crate`];
    assert.deepEqual(filesUnder(directory).sort(), [...[...published.keys()].map((name) => name.slice(prefix.length)), ...extras].sort(), 'vendored tree may contain only published files plus provenance');

    for (const [name, originalBytes] of published) {
      const relative = name.slice(prefix.length);
      const vendored = readFileSync(path.join(directory, relative));
      let expected = originalBytes;
      if (relative === 'Cargo.toml') {
        let source = expected.toString('utf8');
        source = replaceOnce(source, `version = "${crate.version}"\n`, `version = "${crate.version}"\npublish = false\n`, `${crate.package} package version`);
        source = replaceOnce(source, crate.oldDependency, crate.newDependency, `${crate.package} normalized dependency`);
        expected = Buffer.from(source);
      } else if (relative === 'Cargo.toml.orig') {
        let source = expected.toString('utf8');
        source = replaceOnce(source, `version = "${crate.version}"\n`, `version = "${crate.version}"\npublish = false\n`, `${crate.package} original package version`);
        source = replaceOnce(source, 'proc-macro-error = "1.0"', 'proc-macro-error3 = { version = "3.1.1", default-features = false, features = ["syn2-error"] }', `${crate.package} original dependency`);
        expected = Buffer.from(source);
      } else if (relative === 'src/lib.rs') {
        const source = expected.toString('utf8');
        let patched = replaceOnce(source, '// Take a look at the license at the top of the repository in the LICENSE file.\n', '// Take a look at the license at the top of the repository in the LICENSE file.\n\nextern crate proc_macro_error3 as proc_macro_error;\n', `${crate.package} import alias`);
        if (crate.package === 'glib-macros') patched = replaceOnce(patched, '///     \n', '///\n', 'glib-macros rustdoc whitespace normalization');
        expected = Buffer.from(patched);
      } else if (relative === 'LICENSE') {
        const source = expected.toString('utf8');
        expected = Buffer.from(replaceOnce(source, 'SOFTWARE.\n\n', 'SOFTWARE.\n', `${crate.package} license final blank line`));
      }
      assert.deepEqual(vendored, expected, `unexpected published source modification: ${crate.package}/${relative}`);
    }
  }

  const manifest = readFileSync(path.join(root, 'apps/windows/src-tauri/Cargo.toml'), 'utf8');
  assert.match(manifest, /glib-macros = \{ path = "vendor\/glib-macros-0\.18\.5" \}/);
  assert.match(manifest, /gtk3-macros = \{ path = "vendor\/gtk3-macros-0\.18\.2" \}/);
  const lock = readFileSync(path.join(root, 'apps/windows/src-tauri/Cargo.lock'), 'utf8');
  assert.match(lock, /name = "proc-macro-error3"\nversion = "3\.1\.1"\nsource = "registry\+https:\/\/github\.com\/rust-lang\/crates\.io-index"\nchecksum = "8f0d4471b3436c22106b21913b1dda531558918ae9b7ec55d58aa84b43552233"/);
  assert.doesNotMatch(lock, /name = "proc-macro-error"\nversion = "1\.0\.4"/);
});
