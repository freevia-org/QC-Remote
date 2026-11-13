import { readFile } from 'node:fs/promises';
import {
  getWithRetry,
  validateWebsiteSync,
  websiteUrl,
} from './website-sync-contract.mjs';

const readme = await readFile(new URL('../README.md', import.meta.url), 'utf8');

const get = (url) => getWithRetry(url);

const page = await (await get(websiteUrl)).text();
const contract = validateWebsiteSync(readme, page);

for (const imageUrl of contract.imageUrls) {
  const path = new URL(imageUrl).pathname;
  if (!page.includes(`src="${path}"`)) {
    throw new Error(`Website no longer uses README screenshot: ${imageUrl}`);
  }
  const image = await get(imageUrl);
  if (!image.headers.get('content-type')?.startsWith('image/png')) {
    throw new Error(`Screenshot is not served as PNG: ${imageUrl}`);
  }
  // Headers prove it is served as a PNG; do not download the whole image.
  await image.body?.cancel();
}

console.log('QC Remote website and GitHub README share live screenshots, platform downloads, release notes, and Google Play link.');
