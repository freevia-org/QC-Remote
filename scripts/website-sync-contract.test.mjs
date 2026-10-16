import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { validateWebsiteSync } from './website-sync-contract.mjs';

const screenshots = [
  'https://freevia.org/assets/qcremote/windows.png',
  'https://freevia.org/assets/qcremote/android.png',
];
const downloads = [
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.182/app.exe',
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.182/app.apk',
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.186/qc-remote_0.1.186_amd64.deb',
];
const releaseUrls = [
  'https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.182',
  'https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.186',
];
const storeUrl = 'https://play.google.com/store/apps/details?id=org.freevia.qcremote';
const readme = [
  ...screenshots.map((url) => `<img src="${url}" alt="screenshot">`),
  ...downloads,
  ...releaseUrls,
  storeUrl,
].join('\n');
const page = [
  ...screenshots.map((url) => `src="${new URL(url).pathname}"`),
  ...downloads,
  ...releaseUrls,
  storeUrl,
].join('\n');

test('website sync accepts the stable desktop/mobile downloads plus Linux package', () => {
  const contract = validateWebsiteSync(readme, page);
  assert.equal(contract.downloadUrls.length, 3);
  assert.ok(contract.downloadUrls.some((url) => url.endsWith('.deb')));
  assert.equal(contract.releaseUrls.length, 2);
});

test('Linux release draft version, package manifests and attestation stay aligned', () => {
  const appVersion = JSON.parse(readFileSync(new URL('../apps/windows/package.json', import.meta.url), 'utf8')).version;
  const tauriVersion = JSON.parse(readFileSync(new URL('../apps/windows/src-tauri/tauri.conf.json', import.meta.url), 'utf8')).version;
  const packageLock = JSON.parse(readFileSync(new URL('../package-lock.json', import.meta.url), 'utf8'));
  const cargoManifest = readFileSync(new URL('../apps/windows/src-tauri/Cargo.toml', import.meta.url), 'utf8');
  const cargoLock = readFileSync(new URL('../apps/windows/src-tauri/Cargo.lock', import.meta.url), 'utf8');
  const workflow = readFileSync(new URL('../.github/workflows/linux-broker.yml', import.meta.url), 'utf8');
  const cargoVersion = cargoManifest.match(/^version = "([^"]+)"$/m)?.[1];
  const lockedCargoVersion = cargoLock.match(/\[\[package\]\]\nname = "qc-remote"\nversion = "([^"]+)"/)?.[1];

  // Every desktop version source must agree. The Linux workflow's pinned
  // release version is the last Linux release (0.1.187); the source may move
  // ahead of it with Windows releases, and the draft job itself refuses to run
  // until the source and LINUX_RELEASE_VERSION are brought together.
  assert.match(appVersion, /^\d+\.\d+\.\d+$/);
  assert.equal(tauriVersion, appVersion);
  assert.equal(packageLock.packages['apps/windows'].version, appVersion);
  assert.equal(cargoVersion, appVersion);
  assert.equal(lockedCargoVersion, appVersion);
  assert.match(workflow, /LINUX_RELEASE_VERSION: "0\.1\.187"/);
  assert.match(workflow, /qc-remote_\$\{\{\s*env\.LINUX_RELEASE_VERSION\s*\}\}_amd64\.deb/);
  assert.match(workflow, /release_tag="v\$\{LINUX_RELEASE_VERSION\}"/);
  assert.match(workflow, /--target "\$GITHUB_SHA" --draft --prerelease/);
  assert.match(workflow, /make_latest=false/);
  assert.match(workflow, /--source-ref "\$GITHUB_REF" --source-digest "\$GITHUB_SHA"/);
  assert.match(workflow, /Fresh-login and physical QC acceptance are pending/);
});

test('website sync rejects a page that has not been updated with the Linux package', () => {
  assert.throws(
    () => validateWebsiteSync(readme, page.replace(downloads[2], '')),
    /Website is missing README download/,
  );
});

test('website sync rejects losing either existing stable platform link', () => {
  assert.throws(
    () => validateWebsiteSync(readme, page.replace(downloads[0], '')),
    /Website is missing README download/,
  );
  assert.throws(
    () => validateWebsiteSync(readme, page.replace(downloads[1], '')),
    /Website is missing README download/,
  );
});
