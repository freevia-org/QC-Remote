// Security regression tests for the manually dispatched DCO workflow. The job
// reads only GitHub PR metadata and Git commit objects; it never executes PR code.

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const workflows = path.join(root, '.github', 'workflows');
const dco = readFileSync(path.join(workflows, 'dco.yml'), 'utf8');
const helper = readFileSync(path.join(root, 'scripts', 'verify-dco-dispatch.mjs'), 'utf8');

test('DCO runs only by workflow dispatch from protected master', () => {
  assert.match(dco, /^on:\r?\n  workflow_dispatch:/m);
  assert.match(dco, /^      pr_number:\r?\n/m);
  assert.doesNotMatch(dco, /^  (?:push|pull_request|pull_request_target|schedule):/m);
  assert.match(dco, /^    if: github\.ref == 'refs\/heads\/master'$/m);
  assert.match(dco, /^    name: DCO sign-off$/m);
});

test('DCO workflow uses read-only permissions and a pinned base checkout', () => {
  assert.match(dco, /^permissions:\r?\n  contents: read\r?\n  pull-requests: read$/m);
  assert.match(dco, /uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/);
  assert.match(dco, /fetch-depth: 0/);
  assert.doesNotMatch(dco, /\bsecrets\./);
  assert.doesNotMatch(dco, /actions\/(?:cache|upload-artifact|download-artifact)/);
  assert.match(dco, /run: node scripts\/verify-dco-dispatch\.mjs/);
});

test('dispatch helper validates trusted PR metadata before fetching objects', () => {
  assert.match(helper, /repository !== 'freevia-org\/QC-Remote'/);
  assert.match(helper, /\^\[1-9\]\[0-9\]\*\$/);
  assert.match(helper, /pr\.state !== 'open'/);
  assert.match(helper, /pr\.base\?\.repo\?\.full_name !== repository/);
  assert.match(helper, /pr\.base\?\.ref !== 'master'/);
  assert.match(helper, /const baseSha = pr\.base\?\.sha/);
  assert.match(helper, /Authorization: `Bearer \$\{token\}`/);
  assert.match(helper, /signal: AbortSignal\.timeout\(20_000\)/);
  assert.match(helper, /https:\/\/api\.github\.com\/repos/);
  assert.match(helper, /\^\[0-9a-f\]\{40\}\$/);
  assert.match(helper, /new Set\(\['OWNER', 'MEMBER', 'COLLABORATOR'\]\)/);
  assert.match(helper, /pr\.user\?\.type === 'Bot'/);
  assert.match(helper, /DCO check skipped for trusted PR author/);
  assert.ok(helper.indexOf("if (pr.base?.repo?.full_name !== repository || pr.base?.ref !== 'master')") < helper.indexOf('const trusted = new Set('), 'stacked PRs remain rejected before the trusted-author exemption');
  assert.ok(helper.indexOf('const trusted = new Set(') < helper.indexOf("execFileSync('git', [\n      'fetch'"), 'trusted authors are exempted before fetching PR objects');
  assert.match(helper, /refs\/pull\/\$\{number\}\/head:refs\/remotes\/pull\/\$\{number\}\/head/);
  assert.match(helper, /git', \['cat-file', '-e'/);
  assert.match(helper, /scripts\/verify-dco\.mjs/);
  assert.doesNotMatch(helper, /git', \['(?:checkout|switch|reset|merge|rebase)'/);
});

test('no other workflow uses pull_request_target', () => {
  for (const name of readdirSync(workflows).filter((entry) => /\.ya?ml$/.test(entry) && entry !== 'dco.yml')) {
    const text = readFileSync(path.join(workflows, name), 'utf8').replace(/^\s*#.*$/gm, '');
    assert.doesNotMatch(text, /^\s*pull_request_target:/m, `${name} must not use pull_request_target`);
  }
});

// A job without timeout-minutes inherits GitHub's 360-minute default, and on the
// single self-hosted runner one hung job blocks every other workflow.
test('every job in every workflow declares timeout-minutes', () => {
  for (const name of readdirSync(workflows).filter((entry) => /\.ya?ml$/.test(entry))) {
    const lines = readFileSync(path.join(workflows, name), 'utf8').split(/\r?\n/);
    const jobsAt = lines.findIndex((line) => line === 'jobs:');
    assert.notEqual(jobsAt, -1, `${name} declares jobs`);
    const jobs = [];
    for (const line of lines.slice(jobsAt + 1)) {
      if (line && !/^\s/.test(line)) break;
      const header = line.match(/^ {2}([A-Za-z0-9_-]+):\s*$/);
      if (header) jobs.push({ id: header[1], body: [] });
      else if (jobs.length > 0) jobs[jobs.length - 1].body.push(line);
    }
    assert.ok(jobs.length > 0, `${name} has jobs`);
    for (const { id, body } of jobs) {
      if (body.some((line) => /^ {4}uses:/.test(line))) continue; // reusable workflow call
      assert.ok(body.some((line) => /^ {4}timeout-minutes:/.test(line)), `${name}: job ${id} has no timeout-minutes`);
    }
  }
});

test('Windows desktop app checks its declared Rust 1.92 minimum separately from Quad Cortex 1.87', () => {
  const ci = readFileSync(path.join(workflows, 'ci.yml'), 'utf8');
  const appMsrv = ci.slice(
    ci.indexOf('\n  rust-windows-app-msrv:'),
    ci.indexOf('\n  rust-windows-app-static:'),
  );
  assert.ok(appMsrv.length > 0, 'the dedicated desktop app minimum job exists');
  assert.match(appMsrv, /name: Windows desktop app on its Rust minimum \(1\.92\)/);
  assert.match(appMsrv, /toolchain: '1\.92\.0'/);
  assert.match(appMsrv, /cargo \+1\.92\.0 check --locked/);
  assert.match(appMsrv, /cargo \+1\.92\.0 build --locked --features tray-sidecar --bin qc-device-broker/);
  assert.match(appMsrv, /cargo \+1\.92\.0 clippy --locked --all-targets --features tray-sidecar -- -D warnings/);
  assert.match(appMsrv, /binaries\/qc-device-broker-x86_64-pc-windows-msvc\.exe/);
  assert.match(ci, /rust-msrv:[\s\S]*?name: quadcortex on its declared Rust minimum \(1\.87\)[\s\S]*?cargo \+1\.87 test -p quadcortex --locked/);
  assert.match(ci, /ci-result:[\s\S]*?needs:[\s\S]*?- rust-windows-app-msrv/);
});

test('macOS hosted tests run the Mac App Store workspace document writer regressions', () => {
  const macos = readFileSync(path.join(workflows, 'macos-build.yml'), 'utf8');
  const storeWriter = macos.slice(
    macos.indexOf('\n      - name: Test Mac App Store workspace document writer'),
    macos.indexOf('\n      - name: Test Mac App Store native services'),
  );
  assert.ok(storeWriter.length > 0, 'the Store workspace writer test step exists');
  assert.match(storeWriter, /if: matrix\.lane == 'tests'/);
  assert.match(storeWriter, /cargo test --locked --manifest-path src-tauri\/Cargo\.toml/);
  assert.match(storeWriter, /--features desktop-broker,custom-protocol,macos-app-store/);
  assert.match(storeWriter, /--lib workspace::atomic_document_tests --target \$\{\{ matrix\.target \}\}/);
  assert.match(storeWriter, /working-directory: apps\/windows/);
});

// A Python test module or generator check that no workflow runs is a gate that
// silently does not exist.
test('every Python test module and the header-glyph generator check is run by a workflow', () => {
  const all = readdirSync(workflows)
    .filter((entry) => /\.ya?ml$/.test(entry))
    .map((entry) => readFileSync(path.join(workflows, entry), 'utf8'))
    .join('\n');
  for (const file of readdirSync(path.join(root, 'scripts')).filter((entry) => /^test_.*\.py$/.test(entry))) {
    assert.ok(all.includes(`scripts.${file.replace(/\.py$/, '')}`), `${file} is not run by any workflow`);
  }
  assert.match(all, /python3 scripts\/generate-coros-header-font\.py --check/);
});

test('apt-get calls retry transient mirror failures', () => {
  const text = readFileSync(path.join(workflows, 'linux-broker.yml'), 'utf8');
  const calls = text.match(/apt-get [^\n]*/g) ?? [];
  assert.ok(calls.length > 0);
  for (const call of calls) assert.match(call, /-o Acquire::Retries=3/, call);
});

test('Linux release package uses a clean per-attempt target directory', () => {
  const text = readFileSync(path.join(workflows, 'linux-broker.yml'), 'utf8');
  const release = text.slice(text.indexOf('\n  linux-preview-release:'));
  const target = '${{ runner.temp }}/qc-remote-linux-package-${{ github.run_id }}-${{ github.run_attempt }}';
  assert.equal(release.split(target).length - 1, 2);
  assert.match(release, /name: Build and verify Debian package from this source commit[\s\S]*?CARGO_TARGET_DIR:/);
  assert.match(release, /name: Name package and write checksum manifest[\s\S]*?test -d "\$CARGO_TARGET_DIR"[\s\S]*?find "\$CARGO_TARGET_DIR"/);
});

test('Linux D-Bus qualification runs the single-instance forwarding and release tests', () => {
  const workflow = readFileSync(path.join(workflows, 'linux-broker.yml'), 'utf8');
  const hostTest = workflow.slice(
    workflow.indexOf('\n      - name: Test the GUI host, KSNI menu layout and D-Bus recovery'),
    workflow.indexOf('\n      - name: Test the GLib string iterator backport in optimized mode'),
  );
  assert.ok(hostTest.length > 0, 'the Linux GUI-host test step exists');
  assert.match(hostTest, /dbus-run-session -- cargo \+stable test --locked --features tray-sidecar,custom-protocol/);
  assert.match(hostTest, /--test instance_lock --test dbus_errors -- --include-ignored/);
  assert.match(hostTest, /--manifest-path \.\.\/\.\.\/\.\.\/services\/device-broker\/Cargo\.toml --features linux-tray --test linux_ksni_menu_layout/);
  assert.match(workflow, /dbus-run-session -- python3 -m unittest scripts\.test_linux_tray_startup -v/);
});
