import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import {
  CLOUD_ONLY_GLOBS,
  CLOUD_PACKAGE_JSON,
  MARKDOWN_ONLY_GLOBS,
  NATIVE_SKIP_GLOBS,
  SAFE_GLOBS,
  classify,
  isCloudOnlyPath,
  isCloudScriptsOnlyEdit,
  isMarkdownOnlyPath,
  isSafePath,
} from './ci-changes.mjs';

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
  assert.deepEqual(gates(classify(['docs/a.md', 'services/qc-remote-cloud/Cargo.toml'])), { native: true, typescript: true, cloud: true });
  assert.deepEqual(gates(classify(['docs/a.md', 'services/rust-mcp/src/server.rs'])), { native: true, typescript: true, cloud: true });
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

test('linux-broker.yml paths-ignore mirrors the native-skip globs exactly', () => {
  const block = workflow('linux-broker.yml').match(/^  pull_request:\n    paths-ignore:\n((?:      - .*\n)+)/m);
  assert.ok(block, 'linux-broker.yml must declare pull_request paths-ignore');
  const globs = block[1]
    .trim()
    .split('\n')
    .map((line) => line.replace(/^\s*- /, '').replace(/^["']|["']$/g, ''));
  assert.deepEqual(globs, NATIVE_SKIP_GLOBS);
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
  for (const [job, name] of [
    ['native-linux', 'Headless broker on native Linux'],
    ['linux-desktop-host', 'Linux Tauri host and resident tray'],
  ]) {
    const start = text.indexOf(`\n  ${job}:\n`) + 1;
    assert.ok(start > 0, job);
    const rest = text.slice(start);
    const end = rest.slice(1).search(/^ {2}[a-z][a-z0-9-]*:\s*$/m);
    const body = end === -1 ? rest : rest.slice(0, end + 1);
    assert.ok(body.startsWith(`  ${job}:\n    name: ${name}\n`), job);
    assert.match(body, /^    needs: changes$/m, job);
    // Never run on dispatch; on a pull request only when the classifier says native.
    assert.match(
      body,
      /^    if: \$\{\{ !cancelled\(\) && github\.event_name != 'workflow_dispatch' && \(github\.event_name != 'pull_request' \|\| needs\.changes\.result != 'success' \|\| needs\.changes\.outputs\.native != 'false'\) \}\}$/m,
      job,
    );
  }
  assert.match(text, /^  changes:\n    name: Select Linux jobs\n    if: github\.event_name == 'pull_request'\n/m);
  assert.match(text, /^        run: node scripts\/ci-changes\.mjs$/m);
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

// ---------------------------------------------------------------------------
// Cloud relay (services/qc-remote-cloud) native independence.
// ---------------------------------------------------------------------------

// Exact file list of public PR #179 (freevia-org/QC-Remote), "Retain the
// emergency MCP guard during quota rollout": gh pr view 179 --json files.
const PR_179_FILES = [
  'services/qc-remote-cloud/package.json',
  'services/qc-remote-cloud/scripts/rate-limit.test.mjs',
  'services/qc-remote-cloud/src/rate-limit.ts',
  'services/qc-remote-cloud/worker-configuration.d.ts',
  'services/qc-remote-cloud/wrangler.jsonc',
];

// package.json of that PR, before and after: only the `test` script changed.
const PR_179_PACKAGE_BEFORE = JSON.stringify({
  name: '@qc-remote/cloud',
  scripts: { check: 'x', deploy: 'y', test: 'node --test scripts/apply-kv-namespace-id.test.mjs' },
  dependencies: { zod: '^4.6.7' },
});
const PR_179_PACKAGE_AFTER = JSON.stringify({
  name: '@qc-remote/cloud',
  scripts: {
    check: 'x',
    deploy: 'y',
    test: 'node --test scripts/apply-kv-namespace-id.test.mjs && node --experimental-strip-types --test scripts/rate-limit.test.mjs',
  },
  dependencies: { zod: '^4.6.7' },
});

const scriptsOnly = (value) => ({ cloudPackageJsonEdit: () => value });

test('PR 179 (relay guard only) runs the relay and TypeScript jobs and skips every native job', () => {
  assert.equal(isCloudScriptsOnlyEdit(PR_179_PACKAGE_BEFORE, PR_179_PACKAGE_AFTER), true);
  assert.deepEqual(gates(classify(PR_179_FILES, scriptsOnly(true))), { native: false, typescript: true, cloud: true });
  // Without proof that package.json is a scripts-only edit, fail closed.
  assert.deepEqual(gates(classify(PR_179_FILES)), { native: true, typescript: true, cloud: true });
  assert.deepEqual(gates(classify(PR_179_FILES, scriptsOnly(false))), { native: true, typescript: true, cloud: true });
});

test('relay sources, tests, wrangler and type files are native independent; nothing else under the service is', () => {
  for (const file of [
    'services/qc-remote-cloud/src/index.ts',
    'services/qc-remote-cloud/src/rate-limit.ts',
    'services/qc-remote-cloud/src/nested/dir/a.ts',
    'services/qc-remote-cloud/tests/workers-runtime/kv-proxy.test.ts',
    'services/qc-remote-cloud/scripts/apply-kv-namespace-id.mjs',
    'services/qc-remote-cloud/wrangler.jsonc',
    'services/qc-remote-cloud/wrangler.kv-proxy-test.jsonc',
    'services/qc-remote-cloud/worker-configuration.d.ts',
    'services/qc-remote-cloud/tsconfig.json',
    'services/qc-remote-cloud/tsconfig.workers-tests.json',
    'services/qc-remote-cloud/vitest.kv-proxy.config.ts',
    'services/qc-remote-cloud/README.md',
  ]) assert.equal(isCloudOnlyPath(file), true, file);
  for (const file of [
    // package.json is lockfile input for the Android job's npm ci: never by name.
    CLOUD_PACKAGE_JSON,
    'services/qc-remote-cloud/package-lock.json',
    'services/qc-remote-cloud/.npmrc',
    // A crate or native source dropped into the service is not a relay source.
    'services/qc-remote-cloud/Cargo.toml',
    'services/qc-remote-cloud/src/lib.rs',
    'services/qc-remote-cloud/build.rs',
    'services/qc-remote-cloud/src/data.json',
    'services/qc-remote-cloud/src/schema.proto',
    'services/qc-remote-cloud/scripts/run.ps1',
    'services/qc-remote-cloud/scripts/run.sh',
    'services/qc-remote-cloud/node_modules/x/src/a.ts',
    // Neighbours and shared inputs.
    'services/qc-remote-cloud-evil/src/a.ts',
    'services/qc-remote-cloudx/src/a.ts',
    'services/rust-mcp/tests/fixtures/model-query-cases.json',
    'services/rust-mcp/tests/fixtures/schema-validator-cases.json',
    'services/rust-mcp/src/server.rs',
    'services/device-broker/src/lib.rs',
    'contracts/public/qc-actions.v1.json',
    'packages/typescript/qc-core/tests/mcp-adapter.test.ts',
    'packages/typescript/qc-theme/src/colors.json',
    'package.json',
    'package-lock.json',
  ]) assert.equal(isCloudOnlyPath(file), false, file);
});

test('the native-skip globs only widen the safe set by the relay sources', () => {
  assert.deepEqual(NATIVE_SKIP_GLOBS, [...SAFE_GLOBS, ...CLOUD_ONLY_GLOBS]);
  assert.ok(CLOUD_ONLY_GLOBS.every((glob) => glob.startsWith('services/qc-remote-cloud/')));
});

test('a relay-only diff keeps the relay and TypeScript jobs and skips native; mixed with a native input it runs native', () => {
  const src = 'services/qc-remote-cloud/src/index.ts';
  assert.deepEqual(gates(classify([src])), { native: false, typescript: true, cloud: true });
  assert.deepEqual(gates(classify([src, 'services/qc-remote-cloud/tests/workers-runtime/kv-proxy.test.ts'])), {
    native: false,
    typescript: true,
    cloud: true,
  });
  // With ordinary safe files the relay still runs (cloud follows the relay files).
  assert.deepEqual(gates(classify([src, 'docs/USER-GUIDE.md', 'legal/THIRD_PARTY-LICENSE-TEXTS.txt'])), {
    native: false,
    typescript: true,
    cloud: true,
  });
  // Each native input, however small, brings every native job back.
  for (const nativeInput of [
    'services/rust-mcp/tests/fixtures/model-query-cases.json',
    'services/rust-mcp/tests/fixtures/schema-validator-cases.json',
    'services/device-broker/src/lib.rs',
    'contracts/public/qc-actions.v1.json',
    'package.json',
    'package-lock.json',
    'Cargo.lock',
    'packages/rust/qc-relay-protocol/src/lib.rs',
    'packages/typescript/qc-core/src/index.ts',
    'packages/typescript/qc-theme/src/colors.json',
    'apps/windows/src-tauri/src/lib.rs',
    'apps/android/android/app/build.gradle',
    '.github/workflows/ci.yml',
    'scripts/ci-changes.mjs',
    'services/qc-remote-cloud/Cargo.toml',
  ]) {
    assert.equal(classify([src, nativeInput], scriptsOnly(true)).native, true, nativeInput);
  }
  // A legal-only diff is unchanged: relay skipped too.
  assert.deepEqual(gates(classify(['legal/THIRD_PARTY-LICENSE-TEXTS.txt'])), { native: false, typescript: true, cloud: false });
  assert.deepEqual(gates(classify(['README.md'])), { native: false, typescript: false, cloud: false });
});

test('services/qc-remote-cloud/package.json is skippable only for a non-lifecycle scripts edit', () => {
  const base = { name: '@qc-remote/cloud', version: '0.1.0', scripts: { test: 'a', check: 'b' }, dependencies: { zod: '^4.6.7' } };
  const edit = (change) => isCloudScriptsOnlyEdit(JSON.stringify(base), JSON.stringify(change(structuredClone(base))));
  assert.equal(edit((pkg) => pkg), true, 'no-op');
  assert.equal(edit((pkg) => ({ ...pkg, scripts: { ...pkg.scripts, test: 'a && c' } })), true);
  assert.equal(edit((pkg) => ({ ...pkg, scripts: { ...pkg.scripts, 'test:workers': 'vitest' } })), true);
  // Dependency, version, name, workspaces and overrides edits move the lockfile or npm ci.
  assert.equal(edit((pkg) => ({ ...pkg, dependencies: { zod: '^5.0.0' } })), false);
  assert.equal(edit((pkg) => ({ ...pkg, dependencies: { ...pkg.dependencies, left: '1.0.0' } })), false);
  assert.equal(edit((pkg) => ({ ...pkg, devDependencies: { wrangler: '4.0.0' } })), false);
  assert.equal(edit((pkg) => ({ ...pkg, overrides: { x: '1' } })), false);
  assert.equal(edit((pkg) => ({ ...pkg, version: '0.1.1' })), false);
  assert.equal(edit((pkg) => ({ ...pkg, name: '@qc-remote/other' })), false);
  assert.equal(edit((pkg) => ({ ...pkg, type: 'module' })), false);
  // Lifecycle scripts run during npm ci / npm run build --workspaces.
  for (const name of ['postinstall', 'install', 'preinstall', 'prepare', 'prepublishOnly', 'build', 'prebuild', 'postbuild', 'version']) {
    assert.equal(edit((pkg) => ({ ...pkg, scripts: { ...pkg.scripts, [name]: 'node x.js' } })), false, name);
  }
  assert.equal(edit((pkg) => ({ ...pkg, scripts: { check: 'b' } })), true, 'removing test is allowed');
  assert.equal(edit((pkg) => ({ ...pkg, scripts: { ...pkg.scripts, postinstall: undefined } })), true, 'a dropped undefined key is a no-op');
  // Unparseable or non-object input fails closed.
  assert.equal(isCloudScriptsOnlyEdit('{', JSON.stringify(base)), false);
  assert.equal(isCloudScriptsOnlyEdit(JSON.stringify(base), ''), false);
  assert.equal(isCloudScriptsOnlyEdit('[]', '[]'), false);
  assert.equal(isCloudScriptsOnlyEdit('null', 'null'), false);
  assert.equal(isCloudScriptsOnlyEdit(JSON.stringify({ ...base, scripts: 'x' }), JSON.stringify(base)), false);
});

test('mutation check: the PR 179 expectation fails if the relay globs or the package.json proof are dropped', () => {
  const relayGlobsRemoved = classify(PR_179_FILES.filter((file) => file !== CLOUD_PACKAGE_JSON && !isCloudOnlyPath(file)), scriptsOnly(true));
  // Every PR 179 file but package.json is a relay source, so the list collapses to nothing and fails closed.
  assert.equal(relayGlobsRemoved.native, true);
  // Treating package.json as skippable by name would also skip a dependency change.
  const depBefore = JSON.stringify({ name: '@qc-remote/cloud', dependencies: { zod: '^4.6.7' } });
  const depAfter = JSON.stringify({ name: '@qc-remote/cloud', dependencies: { zod: '^4.7.0' } });
  assert.equal(isCloudScriptsOnlyEdit(depBefore, depAfter), false);
  assert.equal(classify([CLOUD_PACKAGE_JSON, 'services/qc-remote-cloud/src/index.ts'], scriptsOnly(isCloudScriptsOnlyEdit(depBefore, depAfter))).native, true);
});

test('relay-only classification is pinned to the workflow wiring that makes it safe', () => {
  const ci = workflow('ci.yml');
  const jobsText = ci.slice(ci.indexOf('\njobs:\n') + 7);
  const bodyOf = (name) => {
    const start = jobsText.indexOf(`\n  ${name}:\n`);
    assert.notEqual(start, -1, `${name} is missing`);
    const rest = jobsText.slice(start + 1);
    const next = rest.slice(1).search(/^ {2}[a-z][a-z0-9-]*:\s*$/m);
    return next === -1 ? rest : rest.slice(0, next + 1);
  };
  const nativeGate = /^ {4}if: \$\{\{ !cancelled\(\) && \(needs\.changes\.result != 'success' \|\| needs\.changes\.outputs\.native != 'false'\) \}\}$/m;
  for (const job of ['rust', 'rust-msrv', 'rust-windows-app-static', 'rust-windows-app-tests', 'quadcortex-linux', 'android']) {
    assert.match(bodyOf(job), nativeGate, `${job} must be gated on the native output`);
  }
  // The relay and TypeScript jobs are what a relay-only diff keeps.
  assert.match(bodyOf('cloud-relay'), /needs\.changes\.outputs\.cloud != 'false'/);
  assert.match(bodyOf('typescript'), /needs\.changes\.outputs\.typescript != 'false'/);
  // The always-run required jobs and the aggregate still report for a relay-only diff.
  assert.doesNotMatch(bodyOf('third-party-notices'), /needs\.changes/);
  assert.doesNotMatch(bodyOf('public-history'), /needs\.changes/);
  // The relay's only cross-workspace readers run in the TypeScript job (root npm test and contracts:check).
  assert.match(bodyOf('typescript'), /npm run contracts:check/);
  assert.match(bodyOf('typescript'), /run: npm test$/m);
  // ios and macOS path lists never mention the service, so a relay diff cannot start them.
  for (const name of ['ios.yml', 'macos-build.yml']) {
    assert.doesNotMatch(workflow(name), /qc-remote-cloud/, name);
  }
});
