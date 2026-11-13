// A gate script that decides "am I the entry point?" by comparing URLs exactly
// is skipped, with exit code 0 and no output, when it is reached through a
// directory junction or symlink (Node resolves import.meta.url to the target
// while argv[1] keeps the link). These tests run each script through a link and
// require evidence that main() actually ran.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const scriptsDir = path.dirname(fileURLToPath(import.meta.url));

function throughLink(script, args, body) {
  const holder = mkdtempSync(path.join(tmpdir(), 'qc-entry-link-'));
  const link = path.join(holder, 'scripts-link');
  try {
    symlinkSync(scriptsDir, link, process.platform === 'win32' ? 'junction' : 'dir');
    const result = spawnSync(process.execPath, [path.join(link, script), ...args], {
      encoding: 'utf8',
      timeout: 240_000,
    });
    body(result);
  } finally {
    rmSync(holder, { recursive: true, force: true });
  }
}

test('the licence carve-out gate runs when reached through a link', () => {
  throughLink('verify-license-carveout.mjs', [], ({ status, stdout, stderr }) => {
    assert.equal(status, 0, stderr);
    assert.match(stdout, /LICENSE schedules name only existing files/);
  });
});

test('the third-party notices gate compares real paths (its full run takes minutes, so check the guard)', () => {
  // Running it for real invokes cargo metadata for several manifests; the guard
  // is the only thing under test here, so pin its form instead.
  const source = readFileSync(path.join(scriptsDir, 'verify-third-party-notices.mjs'), 'utf8');
  assert.match(source, /fs\.realpathSync\(process\.argv\[1\]\) === fs\.realpathSync\(fileURLToPath\(import\.meta\.url\)\)\) main\(\)/);
  assert.doesNotMatch(source, /import\.meta\.url === pathToFileURL/);
});

test('the Windows native test driver runs when reached through a link', () => {
  throughLink('test-windows-native.mjs', ['--root', path.join(tmpdir(), 'qc-no-such-root')], ({ status, stderr }) => {
    assert.notEqual(status, 0);
    assert.notEqual(stderr.trim(), '', 'main() did not run');
  });
});
