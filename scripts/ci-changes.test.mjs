import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { MARKDOWN_ONLY_GLOBS, SAFE_GLOBS, classify, isMarkdownOnlyPath, isSafePath } from './ci-changes.mjs';

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

const gates = (result) => ({ native: result.native, typescript: result.typescript, cloud: result.cloud });

test('legal records are safe: the Rust code only embeds their bytes', () => {
  for (const file of [
    'legal/THIRD_PARTY-LICENSE-INVENTORY.json',
    'legal/THIRD_PARTY-LICENSE-TEXTS.txt',
    'legal/THIRD_PARTY-SOURCE-OFFER.md',
    'legal/LICENSE-MIT',
    'LICENSE',
    'LICENSE-MIT',
    'LICENSE-APACHE',
    'LICENSES/LicenseRef-QC-Remote-Reference.txt',
    'THIRD_PARTY-NOTICES.md',
    'skills/qc-device-knowledge/LICENSE',
    'skills/qc-device-knowledge/legal/THIRD_PARTY-LICENSE-TEXTS.txt',
    'skills/qc-device-knowledge/THIRD_PARTY-NOTICES.md',
  ]) assert.equal(isSafePath(file), true, file);
});

test('a legal-only change skips native and the relay but still builds TypeScript', () => {
  // The pull request that motivated this: two generated legal records, nothing else.
  const result = classify(['legal/THIRD_PARTY-LICENSE-INVENTORY.json', 'legal/THIRD_PARTY-LICENSE-TEXTS.txt']);
  assert.deepEqual(gates(result), { native: false, typescript: true, cloud: false });
});

test('markdown-only changes also skip TypeScript, but nothing a build or test reads as data does', () => {
  assert.deepEqual(MARKDOWN_ONLY_GLOBS, ['*.md', 'docs/**/*.md']);
  for (const file of ['README.md', 'CHANGELOG.md', 'docs/USER-GUIDE.md', 'docs/guide/setup.md']) {
    assert.equal(isMarkdownOnlyPath(file), true, file);
  }
  for (const file of [
    'THIRD_PARTY-NOTICES.md',
    'docs/reference/qc-live-parameter-scales.json',
    'docs/images/screen.png',
    'packages/typescript/qc-ui/README.md',
    'legal/THIRD_PARTY-SOURCE-OFFER.md',
  ]) assert.equal(isMarkdownOnlyPath(file), false, file);
  assert.deepEqual(gates(classify(['README.md', 'docs/USER-GUIDE.md'])), { native: false, typescript: false, cloud: false });
  assert.equal(classify(['README.md', 'docs/reference/x.json']).typescript, true);
  assert.equal(classify(['README.md', 'legal/THIRD_PARTY-LICENSE-TEXTS.txt']).typescript, true);
});

test('one unsafe file runs every job', () => {
  assert.deepEqual(gates(classify(['docs/a.md', 'services/qc-remote-cloud/src/index.ts'])), { native: true, typescript: true, cloud: true });
  assert.deepEqual(gates(classify([])), { native: true, typescript: true, cloud: true });
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
    'packages/rust/quadcortex/docs/guide/index.md',
    'packages/rust/quadcortex/README.md',
    'services/rust-mcp/README.md',
    // What decides WHICH legal file is embedded or verified is code, not a record.
    'apps/windows/src-tauri/src/client_packages/device_knowledge_skill.rs',
    'apps/windows/src-tauri/tauri.conf.json',
    'scripts/verify-license-carveout.mjs',
    'scripts/verify-third-party-notices.mjs',
    'skills/qc-device-knowledge/SKILL.md',
    'skills/qc-device-knowledge/references/data/qc-model-lineage.json',
    'skills/qc-device-knowledge/CONTRIBUTING.md',
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
      const lines = text.split('\n').filter((line) => line.trim() === `- "!${glob}"`);
      assert.equal(lines.length, 2, `${name} must negate ${glob} for pull_request and push`);
    }
  }
});

test('macOS builds do not trigger on legal records and the Intel runner is not built on pull requests', () => {
  const text = workflow('macos-build.yml');
  for (const entry of ['THIRD_PARTY-NOTICES.md', 'legal/**', 'LICENSE', 'LICENSE-MIT', 'LICENSE-APACHE', 'LICENSES/**']) {
    assert.equal(
      text.split('\n').some((line) => line.trim() === `- ${entry}`),
      false,
      `macos-build.yml still lists ${entry}`,
    );
  }
  const matrix = text.match(/^      matrix: \$\{\{ fromJSON\(github\.event_name == 'pull_request' && '(.*?)' \|\| '(.*?)'\) \}\}$/m);
  assert.ok(matrix, 'the matrix must depend on the event');
  assert.deepEqual(JSON.parse(matrix[1]).include.map((entry) => entry.target), ['aarch64-apple-darwin']);
  assert.deepEqual(JSON.parse(matrix[2]).include.map((entry) => entry.target), ['aarch64-apple-darwin', 'x86_64-apple-darwin']);
});

test('the Linux release job verifies the push CI result instead of repeating it, and caches cargo', () => {
  const text = workflow('linux-broker.yml');
  const release = text.slice(text.indexOf('\n  linux-preview-release:'));
  assert.doesNotMatch(release, /^    needs:/m, 'the release job must not re-run the two check jobs');
  assert.match(release, /name: Require the push CI result for this exact commit/);
  assert.match(release, /Headless broker on native Linux/);
  assert.match(release, /Linux Tauri host and resident tray/);
  assert.match(release, /^      checks: read$/m);
  assert.match(release, /Swatinem\/rust-cache@[0-9a-f]{40} # v[\d.]+\n\s+with:\n\s+workspaces: apps\/windows\/src-tauri -> target/);
  assert.match(text, /native-linux:\n    name: Headless broker on native Linux\n    if: github\.event_name != 'workflow_dispatch'/);
  assert.match(text, /linux-desktop-host:\n    name: Linux Tauri host and resident tray\n    if: github\.event_name != 'workflow_dispatch'/);
});

test('CI result aggregates every other CI job so one required context cannot deadlock', () => {
  const ci = workflow('ci.yml');
  const jobsText = ci.slice(ci.indexOf('\njobs:\n') + 7);
  const jobs = [...jobsText.matchAll(/^ {2}([a-z][a-z0-9-]*):\s*$/gm)].map((match) => match[1]);
  const aggregate = jobsText.slice(jobsText.indexOf('\n  ci-result:'));
  assert.match(aggregate, /^ {4}if: \$\{\{ always\(\) \}\}$/m, 'the aggregate must always run');
  const needs = aggregate.match(/^ {4}needs:\n((?: {6}- .*\n)+)/m);
  assert.ok(needs, 'ci-result must declare needs');
  const listed = needs[1].trim().split('\n').map((line) => line.replace(/^\s*- /, ''));
  assert.deepEqual([...listed].sort(), jobs.filter((job) => job !== 'changes' && job !== 'ci-result').sort());
});

// The files the 0.1.189 / 0.3.39 release preparation (public PR #168) changed.
// A version bump edits manifests, lockfiles, a Gradle file and a plist that the
// native jobs read, so it must keep running every job; only a records-only
// change may skip them.
const VERSION_BUMP_FILES = [
  'CHANGELOG.md',
  'apps/android/android/app/build.gradle',
  'apps/android/package.json',
  'apps/windows/macos-broker/Info.plist',
  'apps/windows/package.json',
  'apps/windows/src-tauri/Cargo.lock',
  'apps/windows/src-tauri/Cargo.toml',
  'apps/windows/src-tauri/tauri.conf.json',
  'package-lock.json',
];

test('a version bump still runs the native, TypeScript and relay jobs', () => {
  const result = classify(VERSION_BUMP_FILES);
  assert.equal(result.native, true);
  assert.equal(result.typescript, true);
  assert.equal(result.cloud, true);
  // Each manifest alone keeps the jobs on; the changelog alone does not.
  for (const file of VERSION_BUMP_FILES.filter((entry) => entry !== 'CHANGELOG.md')) {
    assert.equal(classify(['CHANGELOG.md', file]).native, true, file);
  }
  assert.equal(classify(['CHANGELOG.md']).native, false);
});

test('the Windows app runs as parallel static and test jobs behind the unchanged required context', () => {
  const ci = workflow('ci.yml');
  const jobsText = ci.slice(ci.indexOf('\njobs:\n') + 7);
  const bodyOf = (name) => {
    const start = jobsText.indexOf(`\n  ${name}:\n`);
    assert.notEqual(start, -1, `${name} is missing`);
    const rest = jobsText.slice(start + 1);
    const next = rest.slice(1).search(/^ {2}[a-z][a-z0-9-]*:\s*$/m);
    return next === -1 ? rest : rest.slice(0, next + 1);
  };
  const aggregate = bodyOf('rust-windows-app');
  assert.match(aggregate, /^ {4}name: Windows app \(Rust backend\)$/m, 'the required context keeps its name');
  assert.match(aggregate, /^ {4}needs:\n {6}- rust-windows-app-static\n {6}- rust-windows-app-tests$/m);
  assert.match(aggregate, /^ {4}if: \$\{\{ !cancelled\(\) \}\}$/m, 'it must report even when a Windows job failed');
  assert.match(aggregate, /\.result == "success" or \.result == "skipped"/);
  assert.doesNotMatch(aggregate, /cargo|node scripts/, 'the aggregate does no build work');

  const tests = bodyOf('rust-windows-app-tests');
  assert.match(tests, /node scripts\/test-windows-native\.mjs --root \./);
  // The test script builds the broker from the root workspace, so that target must be cached.
  assert.match(tests, /workspaces: \|\n\s+\. -> target\n\s+apps\/windows\/src-tauri -> target/);
  assert.match(tests, /shared-key: rust-windows-app$/m, 'the test job inherits the existing master cache key');

  const stat = bodyOf('rust-windows-app-static');
  for (const step of [
    'cargo fmt --check',
    'cargo check --locked',
    'cargo build --locked --features tray-sidecar --bin qc-device-broker',
    'cargo clippy --locked --all-targets --features tray-sidecar -- -D warnings',
  ]) {
    assert.ok(stat.includes(step), `static job must keep: ${step}`);
  }
  assert.doesNotMatch(stat, /test-windows-native\.mjs --root/, 'the native tests run in the tests job only');
  assert.doesNotMatch(tests, /cargo clippy|cargo fmt|cargo check/, 'static checks run in the static job only');
});
