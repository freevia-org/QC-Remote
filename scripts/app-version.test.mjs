import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, mkdtempSync, mkdirSync, writeFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { androidVersionCode, appVersionProblems, planAppVersion } from './app-version.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const read = path => readFileSync(join(root, path), 'utf8');
const project = 'apps/ios/ios/App/App.xcodeproj/project.pbxproj';
const gradle = 'apps/android/android/app/build.gradle';
const canonical = JSON.parse(read('release-version.json')).canonical;
const [currentMarketing, currentBuild] = canonical.split('+');
const [major, minor, releaseCode] = currentMarketing.split('.').map(Number);
// Exercise a marketing and build bump without assuming a particular checked-in
// release. The generator's documented bounds require revisiting the version
// scheme before either component reaches its maximum.
const nextMarketing = `${major}.${minor}.${releaseCode + 1}`;
const nextBuild = String(Number(currentBuild) + 1);
const next = `${nextMarketing}+${nextBuild}`;

test('the checked-in application versions agree on all platforms', () => {
  assert.deepEqual(appVersionProblems(read), []);
});

test('one coordinated bump derives platform identifiers from MAJOR.MINOR.CODE+BUILD and is idempotent', () => {
  const plan = planAppVersion(read, next);
  const updated = path => plan.get(path) ?? read(path);
  assert.deepEqual(appVersionProblems(updated), []);
  for (const platform of ['windows', 'android', 'ios']) {
    const before = JSON.parse(read(`apps/${platform}/package.json`));
    const after = JSON.parse(updated(`apps/${platform}/package.json`));
    assert.equal(after.version, nextMarketing);
    assert.deepEqual({ ...after, version: before.version }, before);
  }
  assert.equal(JSON.parse(updated('release-version.json')).canonical, next);
  assert.equal(JSON.parse(updated('apps/windows/src-tauri/tauri.conf.json')).bundle.resources['../../../release-version.json'], 'release/release-version.json');
  const code = text => Number(/versionCode\s+(\d+)/.exec(text)[1]);
  assert.equal(code(updated(gradle)), androidVersionCode(next));
  assert.equal(code(updated(gradle)), major * 100_000_000 + minor * 1_000_000 + (releaseCode + 1) * 1_000 + Number(nextBuild));
  assert.equal(code(updated(gradle)), androidVersionCode(JSON.parse(updated('release-version.json')).canonical));
  const builds = text => [...text.matchAll(/CURRENT_PROJECT_VERSION = ([^;]+);/g)].map(m => m[1]);
  assert.deepEqual(builds(updated(project)), ['1', '1', nextBuild, nextBuild]);
  const appStoreConfig = JSON.parse(updated('apps/windows/src-tauri/tauri.appstore.conf.json'));
  assert.equal(appStoreConfig.bundle.macOS.bundleVersion, nextBuild);
  assert.equal(appStoreConfig.bundle.macOS.files['Resources/release-version.json'], '../../../release-version.json');
  const broker = updated('apps/windows/macos-broker/Info.plist');
  assert.match(broker, new RegExp(`<key>CFBundleShortVersionString</key>\\s*<string>${nextMarketing.replaceAll('.', '\\.')}<\\/string>`));
  assert.match(broker, new RegExp(`<key>CFBundleVersion</key>\\s*<string>${nextBuild}<\\/string>`));
  assert.equal(planAppVersion(updated, next).size, 0);
});

test('Android versionCode mapping preserves tuple order, fits Play limits and rejects invalid canonical values', () => {
  assert.equal(androidVersionCode('1.0.6+198'), 100_006_198);
  assert.ok(androidVersionCode('1.1.0+1') > androidVersionCode('1.0.999+999'));
  assert.ok(androidVersionCode('2.0.0+1') > androidVersionCode('1.99.999+999'));
  assert.equal(androidVersionCode('20.99.999+999'), 2_099_999_999);
  for (const invalid of ['1.100.0+1', '1.0.1000+1', '1.0.1+1000', '21.0.0+1', '1.0.6', '1.0.6+0']) {
    assert.throws(() => androidVersionCode(invalid));
  }
});

test('drift in Android, iOS native settings, macOS broker, or workspace lock is rejected', () => {
  const currentMarketing = canonical.split('+')[0];
  const currentBuild = canonical.split('+')[1];
  for (const [path, mutate, label] of [
    ['apps/android/package.json', t => t.replace(`"${currentMarketing}"`, '"8.8.8"'), 'android package'],
    [gradle, t => t.replace(/versionCode\s+\d+/, 'versionCode 1'), 'Android versionCode'],
    [project, t => t.replace(`MARKETING_VERSION = ${currentMarketing};`, 'MARKETING_VERSION = 8.8.8;'), 'iOS App'],
    [project, t => t.replaceAll(`CURRENT_PROJECT_VERSION = ${currentBuild};`, 'CURRENT_PROJECT_VERSION = 6;'), 'iOS App'],
    ['apps/windows/macos-broker/Info.plist', t => t.replace(`<string>${currentMarketing}</string>`, '<string>8.8.8</string>'), 'macOS broker marketing version'],
    ['apps/windows/src-tauri/tauri.appstore.conf.json', t => t.replace(`"bundleVersion": "${currentBuild}"`, '"bundleVersion": "196"'), 'Mac App Store bundleVersion'],
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
    if (process.platform !== 'win32') {
      const alias = join(temp, 'version-cli.mjs');
      symlinkSync(join(temp, 'scripts/app-version.mjs'), alias);
      assert.notEqual(spawnSync(process.execPath, [alias, 'invalid'], { encoding: 'utf8' }).status, 0);
    }
    for (const version of ['invalid', '1.0.6', '1.0.6+0', '0.0.1+100', '1.0.7+198']) {
      assert.notEqual(run(version).status, 0);
      for (const path of files) assert.equal(readFileSync(join(temp, path), 'utf8'), read(path));
    }
    const bad = read(project).replace('504EC3181FED79650016851F /* Release */ = {', 'MISSING_APP_RELEASE = {');
    assert.notEqual(bad, read(project));
    writeFileSync(join(temp, project), bad);
    assert.notEqual(run(next).status, 0);
    for (const path of files) assert.equal(readFileSync(join(temp, path), 'utf8'), path === project ? bad : read(path));
  } finally { rmSync(temp, { recursive: true, force: true }); }
});
