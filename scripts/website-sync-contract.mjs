export const websiteUrl = 'https://freevia.org/qcremote/';

const storeUrl = 'https://play.google.com/store/apps/details?id=org.freevia.qcremote';

export function validateWebsiteSync(readme, page) {
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

  const downloadUrls = [...readme.matchAll(
    /https:\/\/github\.com\/freevia-org\/QC-Remote\/releases\/download\/[^\s)"<>]+\.(?:exe|apk|deb)/g,
  )].map((match) => match[0]);
  const hasWindows = downloadUrls.some((url) => url.endsWith('.exe'));
  const hasAndroid = downloadUrls.some((url) => url.endsWith('.apk'));
  const linuxDownloads = downloadUrls.filter((url) => url.endsWith('.deb'));
  if (!hasWindows || !hasAndroid || linuxDownloads.length !== 1 || downloadUrls.length !== 3) {
    throw new Error('README must link one Windows installer, one Android APK, and one Linux Debian package.');
  }
  for (const url of downloadUrls) {
    if (!page.includes(url)) throw new Error(`Website is missing README download: ${url}`);
  }

  const releaseUrls = [...readme.matchAll(
    /https:\/\/github\.com\/freevia-org\/QC-Remote\/releases\/tag\/v[\d.]+(?:-[A-Za-z0-9.-]+)?/g,
  )].map((match) => match[0]);
  const uniqueReleaseUrls = [...new Set(releaseUrls)];
  if (uniqueReleaseUrls.length < 2 || uniqueReleaseUrls.some((url) => !page.includes(url))) {
    throw new Error('Website and README must link both the stable release and Linux preview notes.');
  }
  if (!readme.includes(storeUrl) || !page.includes(storeUrl)) {
    throw new Error('Website and README must link the same Google Play listing.');
  }

  return { imageUrls, downloadUrls, releaseUrls: uniqueReleaseUrls };
}

// GET with a bounded number of attempts. Network errors, timeouts and 5xx answers
// are retried (with backoff) because one transient failure of the live site must
// not fail the check; a 4xx answer or a persistent failure still throws.
export async function getWithRetry(
  url,
  { fetchImpl = fetch, attempts = 3, timeoutMs = 20_000, sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms)) } = {},
) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    let retryable = true;
    try {
      const response = await fetchImpl(url, { signal: AbortSignal.timeout(timeoutMs) });
      if (response.ok) return response;
      lastError = new Error(`${url}: HTTP ${response.status}`);
      retryable = response.status >= 500;
      // Release the connection of an answer we are discarding.
      await response.body?.cancel?.().catch(() => {});
    } catch (error) {
      lastError = error;
    }
    if (!retryable) throw lastError;
    if (attempt < attempts) await sleep(1000 * 2 ** (attempt - 1));
  }
  throw lastError;
}
