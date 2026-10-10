import { readFile } from 'node:fs/promises';

const readme = await readFile(new URL('../README.md', import.meta.url), 'utf8');
const websiteUrl = 'https://freevia.org/qcremote/';

async function get(url) {
  const response = await fetch(url, { signal: AbortSignal.timeout(20_000) });
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`);
  return response;
}

const page = await (await get(websiteUrl)).text();
const imageUrls = [...readme.matchAll(/<img src="(https:\/\/freevia\.org\/assets\/qcremote\/[^" ]+\.png)"/g)]
  .map((match) => match[1]);
if (imageUrls.length !== 2) {
  throw new Error('README must show exactly two canonical website screenshots (Windows and Android).');
}
const readmeImagePaths = new Set(imageUrls.map((url) => new URL(url).pathname));
const websiteImagePaths = new Set(
  [...page.matchAll(/src="(\/assets\/qcremote\/[^" ]+\.png)"/g)].map((match) => match[1]),
);
if (websiteImagePaths.size !== 2 || [...websiteImagePaths].some((path) => !readmeImagePaths.has(path))) {
  throw new Error('The website and README do not use the same two presentation screenshots.');
}

for (const imageUrl of imageUrls) {
  const path = new URL(imageUrl).pathname;
  if (!page.includes(`src="${path}"`)) {
    throw new Error(`Website no longer uses README screenshot: ${imageUrl}`);
  }
  const image = await get(imageUrl);
  if (!image.headers.get('content-type')?.startsWith('image/png')) {
    throw new Error(`Screenshot is not served as PNG: ${imageUrl}`);
  }
  await image.arrayBuffer();
}

const downloadUrls = [...readme.matchAll(/https:\/\/github\.com\/freevia-org\/QC-Remote\/releases\/download\/[^\s)"<>]+\.(?:exe|apk)/g)]
  .map((match) => match[0]);
if (downloadUrls.length !== 2) {
  throw new Error('README must link one Windows installer and one Android APK.');
}
for (const url of downloadUrls) {
  if (!page.includes(url)) throw new Error(`Website is missing README download: ${url}`);
}

const releaseUrl = readme.match(/https:\/\/github\.com\/freevia-org\/QC-Remote\/releases\/tag\/v[\d.]+/)?.[0];
if (!releaseUrl || !page.includes(releaseUrl)) {
  throw new Error('Website and README must link the same release notes.');
}

const storeUrl = 'https://play.google.com/store/apps/details?id=org.freevia.qcremote';
if (!readme.includes(storeUrl) || !page.includes(storeUrl)) {
  throw new Error('Website and README must link the same Google Play listing.');
}

console.log('QC Remote website and GitHub README share both live screenshots, downloads, release notes, and Google Play link.');
