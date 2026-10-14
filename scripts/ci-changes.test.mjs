import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { SAFE_GLOBS, classify, isSafePath } from './ci-changes.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const workflow = (name) => readFileSync(path.join(root, '.github', 'workflows', name), 'utf8');

test('qc-ui source, docs and root markdown are safe', () => {
  for (const file of [
    'packages/typescript/qc-ui/src/display/Foo.tsx',
    'packages/typescript/qc-ui/src/display/foo.css',
    'packages/typescript/qc-ui/src/a.ts',
    'packages/typescript/qc-ui/tests/foo.test.ts',
    'packages/typescript/qc-ui/README.md',
    'docs/USER-GUIDE.md',
    'docs/images/screen.png',
    'README.md',
    'CHANGELOG.md',
  ]) assert.equal(isSafePath(file), true, file);
});

test('anything that can feed a native build is not safe', () => {
  for (const file of [
    '.github/workflows/ci.yml',
    'Cargo.toml',
    'Cargo.lock',
    'package.json',
    'package-lock.json',
    'contracts/public/qc-actions.v1.json',
    'scripts/ci-changes.mjs',
    'skills/qc-device-knowledge/SKILL.md',
    'legal/THIRD_PARTY-LICENSE-INVENTORY.json',
    'THIRD_PARTY-NOTICES.md',
    'packages/rust/quadcortex/docs/guide/index.md',
    'packages/rust/quadcortex/README.md',
    'services/rust-mcp/README.md',
    'apps/windows/src-tauri/src/lib.rs',
    'apps/windows/src/App.tsx',
    'apps/android/android/app/build.gradle',
    'packages/typescript/qc-core/src/generated-actions.ts',
    'packages/typescript/qc-theme/src/colors.json',
    'packages/typescript/qc-ui/package.json',
    'packages/typescript/qc-ui/vite-demo-device-assets.mjs',
    'packages/typescript/qc-ui/src/font.woff2',
    'packages/typescript/qc-ui/node_modules/x/index.ts',
    'packages/typescript/qc-uix/src/a.ts',
    'packages/typescript/qc-ui/src/a.ts.rs',
  ]) assert.equal(isSafePath(file), false, file);
});

test('one unsafe file among safe ones runs the native jobs', () => {
  const safe = 'packages/typescript/qc-ui/src/a.css';
  assert.equal(classify([safe, 'docs/a.md']).native, false);
  assert.equal(classify([safe, 'Cargo.lock']).native, true);
  assert.equal(classify([safe, 'packages/rust/quadcortex/src/lib.rs']).native, true);
});

test('doubt runs the native jobs', () => {
  assert.equal(classify([]).native, true);
  assert.equal(classify(undefined).native, true);
});

test('linux-broker.yml paths-ignore mirrors the safe globs exactly', () => {
  const block = workflow('linux-broker.yml').match(/^  pull_request:\n    paths-ignore:\n((?:      - .*\n)+)/m);
  assert.ok(block, 'linux-broker.yml must declare pull_request paths-ignore');
  const globs = block[1]
    .trim()
    .split('\n')
    .map((line) => line.replace(/^\s*- /, '').replace(/^["']|["']$/g, ''));
  assert.deepEqual(globs, SAFE_GLOBS);
});

test('ios and macOS builds drop only the qc-ui source globs from their path filters', () => {
  const qcUi = SAFE_GLOBS.filter((entry) => entry.startsWith('packages/typescript/qc-ui/'));
  for (const name of ['ios.yml', 'macos-build.yml']) {
    const text = workflow(name);
    for (const glob of qcUi) {
      const lines = text.split('\n').filter((line) => line.replace(/["']/g, '').trim() === `- !${glob}`);
      assert.equal(lines.length, 2, `${name} must negate ${glob} for pull_request and push`);
    }
  }
});
