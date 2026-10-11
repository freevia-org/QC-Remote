import test from 'node:test';
import assert from 'node:assert/strict';
import { validateWebsiteSync } from './website-sync-contract.mjs';

const screenshots = [
  'https://freevia.org/assets/qcremote/windows.png',
  'https://freevia.org/assets/qcremote/android.png',
];
const downloads = [
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.182/app.exe',
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.182/app.apk',
  'https://github.com/freevia-org/QC-Remote/releases/download/v0.1.184/qc-remote_0.1.184_amd64.deb',
];
const releaseUrls = [
  'https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.182',
  'https://github.com/freevia-org/QC-Remote/releases/tag/v0.1.184',
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

test('website sync accepts the stable desktop/mobile downloads plus Linux preview', () => {
  const contract = validateWebsiteSync(readme, page);
  assert.equal(contract.downloadUrls.length, 3);
  assert.ok(contract.downloadUrls.some((url) => url.endsWith('.deb')));
  assert.equal(contract.releaseUrls.length, 2);
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
