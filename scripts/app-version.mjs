// Keep one canonical MAJOR.MINOR.CODE+BUILD release identity and derive each
// platform's required marketing/build fields from it.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const canonicalPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\+([1-9]\d*)$/;
const releaseVersionPath = 'release-version.json';
const appProject = 'apps/ios/ios/App/App.xcodeproj/project.pbxproj';
const appStoreConfigPath = 'apps/windows/src-tauri/tauri.appstore.conf.json';
const appConfigs = ['504EC3171FED79650016851F', '504EC3181FED79650016851F'];
const gradlePath = 'apps/android/android/app/build.gradle';
const plistPath = 'apps/windows/macos-broker/Info.plist';
const json = text => JSON.parse(text.replace(/^\uFEFF/, ''));
const formatJson = value => JSON.stringify(value, null, 2) + '\n';

function one(text, pattern, label) {
  const matches = [...text.matchAll(pattern)];
  if (matches.length !== 1) throw new Error(`Expected exactly one ${label}; found ${matches.length}`);
  return matches[0];
}

function appConfig(text, id) {
  return one(text, new RegExp(`(${id} /\\* (?:Debug|Release) \\*/ = \\{)([\\s\\S]*?)(\\n\\t\\t\\};)`, 'g'), `App configuration ${id}`);
}

function parseCanonical(value) {
  const match = canonicalPattern.exec(value);
  if (!match) throw new Error('Use MAJOR.MINOR.CODE+BUILD, for example 1.0.6+198');
  const [major, minor, code, build] = match.slice(1).map(Number);
  if (major > 20 || minor > 99 || code > 999 || build > 999) {
    throw new Error('Supported release bounds are major <= 20, minor <= 99, code <= 999, build <= 999');
  }
  const marketing = `${major}.${minor}.${code}`;
  return { major, minor, code, build, marketing, canonical: `${marketing}+${build}` };
}

function parseMarketing(value) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(value);
  if (!match) throw new Error(`Invalid platform marketing version: ${value}`);
  return match.slice(1).map(Number);
}

function compareMarketing(left, right) {
  const a = parseMarketing(left), b = parseMarketing(right);
  for (let i = 0; i < 3; i++) { const delta = a[i] - b[i]; if (delta) return delta; }
  return 0;
}

export function androidVersionCode(canonical) {
  const { major, minor, code, build } = parseCanonical(canonical);
  const value = major * 100_000_000 + minor * 1_000_000 + code * 1_000 + build;
  if (!Number.isSafeInteger(value) || value < 1 || value > 2_100_000_000) {
    throw new Error('Derived Android versionCode is outside Google Play limits');
  }
  return value;
}

export function appVersionProblems(read) {
  const problems = [];
  let release;
  try { release = parseCanonical(json(read(releaseVersionPath)).canonical); }
  catch (error) { return [error.message]; }
  const marketing = release.marketing;
  const build = String(release.build);
  const check = (label, value, expected) => {
    if (value !== expected) problems.push(`${label}: ${value ?? 'missing'} != ${expected}`);
  };
  for (const platform of ['windows', 'android', 'ios']) {
    check(`${platform} package`, json(read(`apps/${platform}/package.json`)).version, marketing);
    check(`${platform} workspace lock`, json(read('package-lock.json')).packages?.[`apps/${platform}`]?.version, marketing);
  }
  const tauri = json(read('apps/windows/src-tauri/tauri.conf.json'));
  check('Tauri / macOS / Linux', tauri.version, marketing);
  check('Windows/Linux canonical version resource', tauri.bundle?.resources?.['../../../release-version.json'], 'release/release-version.json');
  check('desktop Cargo manifest', one(read('apps/windows/src-tauri/Cargo.toml'), /\[package\][\s\S]*?^version = "([^"]+)"/gm, 'desktop Cargo version')[1], marketing);
  check('desktop Cargo lock', one(read('apps/windows/src-tauri/Cargo.lock'), /\[\[package\]\]\r?\nname = "qc-remote"\r?\nversion = "([^"]+)"/g, 'desktop Cargo lock version')[1], marketing);
  const gradle = read(gradlePath);
  check('Android versionName', one(gradle, /^\s*versionName\s+"([^"]+)"/gm, 'Android versionName')[1], marketing);
  const actualCode = Number(one(gradle, /^\s*versionCode\s+(\d+)/gm, 'Android versionCode')[1]);
  const expectedCode = androidVersionCode(release.canonical);
  if (actualCode !== expectedCode) problems.push(`Android versionCode: ${actualCode} != derived ${expectedCode}`);
  const storeConfig = json(read(appStoreConfigPath));
  check('Mac App Store bundleVersion', storeConfig.bundle?.macOS?.bundleVersion, build);
  check('Mac App Store canonical version resource', storeConfig.bundle?.macOS?.files?.['Resources/release-version.json'], '../../../release-version.json');
  const broker = read(plistPath);
  check('macOS broker marketing version', one(broker, /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/g, 'macOS broker version')[1], marketing);
  check('macOS broker build version', one(broker, /<key>CFBundleVersion<\/key>\s*<string>([^<]+)<\/string>/g, 'macOS broker build version')[1], build);
  for (const id of appConfigs) {
    const body = appConfig(read(appProject), id)[2];
    check(`iOS App ${id} marketing version`, one(body, /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1], marketing);
    check(`iOS App ${id} build version`, one(body, /CURRENT_PROJECT_VERSION = ([^;]+);/g, 'iOS build version')[1], build);
  }
  return problems;
}

// Reserve three decimal slots each for code and build, plus two for minor.
// The maximum supported tuple maps to 2,099,999,999 (< Play's 2.1B cap).
export function planAppVersion(read, canonical) {
  const target = parseCanonical(canonical);
  let current;
  try { current = parseCanonical(json(read(releaseVersionPath)).canonical); }
  catch (error) { throw new Error(`Cannot read current canonical release version: ${error.message}`); }
  if (compareMarketing(target.marketing, current.marketing) < 0) throw new Error('Refusing marketing-version downgrade');
  if (target.build < current.build) throw new Error('Refusing to reuse or lower the per-app build ordinal');
  if (compareMarketing(target.marketing, current.marketing) > 0 && target.build === current.build) {
    throw new Error('A new MAJOR.MINOR.CODE release must also increment BUILD');
  }

  const changes = new Map();
  changes.set(releaseVersionPath, formatJson({ canonical: target.canonical }));
  for (const platform of ['windows', 'android', 'ios']) {
    const path = `apps/${platform}/package.json`, value = json(read(path));
    if (compareMarketing(target.marketing, value.version) < 0) throw new Error(`Refusing ${platform} marketing-version downgrade`);
    value.version = target.marketing; changes.set(path, formatJson(value));
  }
  const lock = json(read('package-lock.json'));
  for (const platform of ['windows', 'android', 'ios']) {
    if (!lock.packages?.[`apps/${platform}`]) throw new Error(`Missing ${platform} workspace lock entry`);
    lock.packages[`apps/${platform}`].version = target.marketing;
  }
  changes.set('package-lock.json', formatJson(lock));
  const tauri = json(read('apps/windows/src-tauri/tauri.conf.json'));
  if (compareMarketing(target.marketing, tauri.version) < 0) throw new Error('Refusing Tauri marketing-version downgrade');
  tauri.bundle ??= {};
  tauri.bundle.resources ??= {};
  tauri.bundle.resources['../../../release-version.json'] = 'release/release-version.json';
  tauri.version = target.marketing; changes.set('apps/windows/src-tauri/tauri.conf.json', formatJson(tauri));
  for (const [path, pattern] of [
    ['apps/windows/src-tauri/Cargo.toml', /(\[package\][\s\S]*?^version = ")[^"]+("[^\n]*)/gm],
    ['apps/windows/src-tauri/Cargo.lock', /(\[\[package\]\]\r?\nname = "qc-remote"\r?\nversion = ")[^"]+("[^\n]*)/g],
  ]) {
    const text = read(path); one(text, pattern, path);
    const oldVersion = one(text, path.endsWith('Cargo.toml') ? /\[package\][\s\S]*?^version = "([^"]+)"/gm : /\[\[package\]\]\r?\nname = "qc-remote"\r?\nversion = "([^"]+)"/g, path)[1];
    if (compareMarketing(target.marketing, oldVersion) < 0) throw new Error(`Refusing ${path} marketing-version downgrade`);
    changes.set(path, text.replace(pattern, (_, before, after) => `${before}${target.marketing}${after}`));
  }
  let gradle = read(gradlePath);
  const name = one(gradle, /^(\s*versionName\s+")([^"]+)(")/gm, 'Android versionName');
  if (compareMarketing(target.marketing, name[2]) < 0) throw new Error('Refusing Android marketing-version downgrade');
  const code = one(gradle, /^(\s*versionCode\s+)(\d+)/gm, 'Android versionCode');
  gradle = gradle.replace(name[0], `${name[1]}${target.marketing}${name[3]}`).replace(code[0], `${code[1]}${androidVersionCode(target.canonical)}`);
  changes.set(gradlePath, gradle);
  const plist = read(plistPath);
  const marketing = one(plist, /(<key>CFBundleShortVersionString<\/key>\s*<string>)([^<]+)(<\/string>)/g, 'macOS broker marketing version');
  const brokerBuild = one(plist, /(<key>CFBundleVersion<\/key>\s*<string>)([^<]+)(<\/string>)/g, 'macOS broker build version');
  changes.set(plistPath, plist.replace(marketing[0], `${marketing[1]}${target.marketing}${marketing[3]}`).replace(brokerBuild[0], `${brokerBuild[1]}${target.build}${brokerBuild[3]}`));
  const storeConfig = json(read(appStoreConfigPath));
  if (!storeConfig.bundle?.macOS) throw new Error('Missing macOS App Store bundle configuration');
  storeConfig.bundle.macOS.files ??= {};
  storeConfig.bundle.macOS.files['Resources/release-version.json'] = '../../../release-version.json';
  storeConfig.bundle.macOS.bundleVersion = String(target.build);
  changes.set(appStoreConfigPath, formatJson(storeConfig));
  let project = read(appProject);
  const configurations = appConfigs.map(id => appConfig(project, id));
  for (const c of configurations) {
    const oldVersion = one(c[2], /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1];
    if (compareMarketing(target.marketing, oldVersion) < 0) throw new Error('Refusing iOS marketing version downgrade');
    const body = c[2].replace(/MARKETING_VERSION = [^;]+;/, `MARKETING_VERSION = ${target.marketing};`).replace(/CURRENT_PROJECT_VERSION = [^;]+;/, `CURRENT_PROJECT_VERSION = ${target.build};`);
    project = project.replace(c[0], c[1] + body + c[3]);
  }
  changes.set(appProject, project);
  const overlay = path => changes.get(path) ?? read(path);
  const problems = appVersionProblems(overlay);
  if (problems.length) throw new Error(problems.join('\n'));
  return new Map([...changes].filter(([path, text]) => text !== read(path)));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
    const read = path => readFileSync(resolve(root, path), 'utf8');
    const request = process.argv[2] ?? '--check';
    if (request === '--check') {
      const problems = appVersionProblems(read);
      if (problems.length) throw new Error(problems.join('\n'));
      console.log(`All QC Remote platforms use ${json(read(releaseVersionPath)).canonical}.`);
    } else {
      const changes = planAppVersion(read, request);
      for (const [path, text] of changes) writeFileSync(resolve(root, path), text);
      console.log(`QC Remote ${request}: ${changes.size} version files synchronized from one canonical release value.`);
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
