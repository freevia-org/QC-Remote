import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { androidVersionCode, appVersionProblems, planAppVersion } from './app-version.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const read = path => readFileSync(join(root, path), 'utf8');
const project = 'apps/ios/ios/App/App.xcodeproj/project.pbxproj';
const gradle = 'apps/android/android/app/build.gradle';
const current = JSON.parse(read('apps/windows/package.json')).version;
const next = current.replace(/\d+$/, value => String(Number(value) + 1));

test('the checked-in application versions agree on all platforms', () => {
  assert.deepEqual(appVersionProblems(read), []);
});

test('one bump derives Android and Apple identifiers from the canonical version and is idempotent', () => {
  const plan = planAppVersion(read, next);
  const updated = path => plan.get(path) ?? read(path);
  assert.deepEqual(appVersionProblems(updated), []);
  for (const platform of ['windows', 'android', 'ios']) {
    const before = JSON.parse(read(`apps/${platform}/package.json`));
    const after = JSON.parse(updated(`apps/${platform}/package.json`));
    assert.equal(after.version, next);
    assert.deepEqual({ ...after, version: before.version }, before);
  }
  const code = text => Number(/versionCode\s+(\d+)/.exec(text)[1]);
  assert.equal(code(updated(gradle)), androidVersionCode(next));
  assert.equal(code(updated(gradle)), androidVersionCode(JSON.parse(updated('apps/android/package.json')).version));
  assert.equal(updated(gradle).replace(/versionName\s+"[^"]+"/, 'VERSION').replace(/versionCode\s+\d+/, 'CODE'), read(gradle).replace(/versionName\s+"[^"]+"/, 'VERSION').replace(/versionCode\s+\d+/, 'CODE'));
  const builds = text => [...text.matchAll(/CURRENT_PROJECT_VERSION = ([^;]+);/g)].map(m => m[1]);
  assert.deepEqual(builds(updated(project)), ['1', '1', next, next]);
  assert.equal(updated(project).replace(/MARKETING_VERSION = [^;]+;/g, 'MARKETING').replace(/CURRENT_PROJECT_VERSION = [^;]+;/g, 'BUILD'), read(project).replace(/MARKETING_VERSION = [^;]+;/g, 'MARKETING').replace(/CURRENT_PROJECT_VERSION = [^;]+;/g, 'BUILD'));
  const appStoreConfig = JSON.parse(updated('apps/windows/src-tauri/tauri.appstore.conf.json'));
  assert.equal(appStoreConfig.bundle.macOS.bundleVersion, next);
  const broker = updated('apps/windows/macos-broker/Info.plist');
  assert.match(broker, new RegExp(`<key>CFBundleVersion</key>\\s*<string>${next.replaceAll('.', '\\.')}<\\/string>`));
  assert.equal(planAppVersion(updated, next).size, 0);
});

test('Android versionCode mapping is ordered, bounded and rejects ambiguous tuples', () => {
  assert.equal(androidVersionCode('1.0.198'), 1000198);
  assert.ok(androidVersionCode('1.1.0') > androidVersionCode('1.0.9999'));
  assert.ok(androidVersionCode('2.0.0') > androidVersionCode('1.99.9999'));
  assert.throws(() => androidVersionCode('1.100.0'), /minor < 100/);
  assert.throws(() => androidVersionCode('1.0.10000'), /build < 10000/);
  assert.throws(() => androidVersionCode('2101.0.0'), /Google Play limits/);
});

test('drift in Android, iOS native settings, macOS broker, or workspace lock is rejected', () => {
  for (const [path, mutate, label] of [
    ['apps/android/package.json', t => t.replace(`"${current}"`, '"8.8.8"'), 'android package'],
    [gradle, t => t.replace(/versionCode\s+\d+/, 'versionCode 1'), 'Android versionCode'],
    [project, t => t.replace(`MARKETING_VERSION = ${current};`, 'MARKETING_VERSION = 8.8.8;'), 'iOS App'],
    [project, t => t.replaceAll(`CURRENT_PROJECT_VERSION = ${current};`, 'CURRENT_PROJECT_VERSION = 6;'), 'iOS App'],
    ['apps/windows/macos-broker/Info.plist', t => t.replace(`<string>${current}</string>`, '<string>8.8.8</string>'), 'macOS broker'],
    ['apps/windows/src-tauri/tauri.appstore.conf.json', t => t.replace(`"bundleVersion": "${current}"`, '"bundleVersion": "1.0.197"'), 'Mac App Store bundleVersion'],
    ['package-lock.json', t => { const x = JSON.parse(t); x.packages['apps/ios'].version = '8.8.8'; return JSON.stringify(x); }, 'ios workspace lock'],
  ]) {
    const changed = mutate(read(path));
    assert.notEqual(changed, read(path), path);
    assert.ok(appVersionProblems(p => p === path ? changed : read(p)).some(p => p.includes(label)), label);
  }
});

test('CLI refuses malformed/downgraded versions and invalid late input without partial writes', () => {
  const temp = mkdtempSync(join(tmpdir(), 'qc-unified-version-'));
  try {
    const files = [...new Set([...planAppVersion(read, next).keys(), 'scripts/app-version.mjs'])];
    for (const path of files) { mkdirSync(dirname(join(temp, path)), { recursive: true }); writeFileSync(join(temp, path), read(path)); }
    const run = version => spawnSync(process.execPath, [join(temp, 'scripts/app-version.mjs'), version], { encoding: 'utf8' });
    for (const version of ['invalid', '0.0.1']) { assert.notEqual(run(version).status, 0); for (const path of files) assert.equal(readFileSync(join(temp, path), 'utf8'), read(path)); }
    const bad = read(project).replace('504EC3181FED79650016851F /* Release */ = {', 'MISSING_APP_RELEASE = {');
    assert.notEqual(bad, read(project));
    writeFileSync(join(temp, project), bad);
    assert.notEqual(run(next).status, 0);
    for (const path of files) assert.equal(readFileSync(join(temp, path), 'utf8'), path === project ? bad : read(path));
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
