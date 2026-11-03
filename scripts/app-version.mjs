// One canonical three-part version drives every public app version and the
// store-required Android/Apple identifiers derived from that version.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
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

export function appVersionProblems(read) {
  const problems = [];
  const expected = json(read('apps/windows/package.json')).version;
  if (!semver.test(expected)) problems.push(`Desktop version is not x.y.z: ${expected}`);
  const check = (label, value) => { if (value !== expected) problems.push(`${label}: ${value ?? 'missing'} != ${expected}`); };
  for (const platform of ['windows', 'android', 'ios']) {
    check(`${platform} package`, json(read(`apps/${platform}/package.json`)).version);
    check(`${platform} workspace lock`, json(read('package-lock.json')).packages?.[`apps/${platform}`]?.version);
  }
  check('Tauri / macOS / Linux', json(read('apps/windows/src-tauri/tauri.conf.json')).version);
  check('desktop Cargo manifest', one(read('apps/windows/src-tauri/Cargo.toml'), /\[package\][\s\S]*?^version = "([^"]+)"/gm, 'desktop Cargo version')[1]);
  check('desktop Cargo lock', one(read('apps/windows/src-tauri/Cargo.lock'), /\[\[package\]\]\r?\nname = "qc-remote"\r?\nversion = "([^"]+)"/g, 'desktop Cargo lock version')[1]);
  const gradle = read(gradlePath);
  check('Android versionName', one(gradle, /^\s*versionName\s+"([^"]+)"/gm, 'Android versionName')[1]);
  try {
    const expectedCode = androidVersionCode(expected);
    const actualCode = Number(one(gradle, /^\s*versionCode\s+(\d+)/gm, 'Android versionCode')[1]);
    if (actualCode !== expectedCode) problems.push(`Android versionCode: ${actualCode} != derived ${expectedCode}`);
  } catch (error) { problems.push(error.message); }
  const storeConfig = json(read(appStoreConfigPath));
  check('Mac App Store bundleVersion', storeConfig.bundle?.macOS?.bundleVersion);
  const broker = read(plistPath);
  check('macOS broker marketing version', one(broker, /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/g, 'macOS broker version')[1]);
  check('macOS broker build version', one(broker, /<key>CFBundleVersion<\/key>\s*<string>([^<]+)<\/string>/g, 'macOS broker build version')[1]);
  for (const id of appConfigs) {
    const body = appConfig(read(appProject), id)[2];
    check(`iOS App ${id} marketing version`, one(body, /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1]);
    check(`iOS App ${id} build version`, one(body, /CURRENT_PROJECT_VERSION = ([^;]+);/g, 'iOS build version')[1]);
  }
  return problems;
}

function compare(left, right) {
  const a = left.split('.').map(Number), b = right.split('.').map(Number);
  for (let i = 0; i < 3; i++) { const delta = (a[i] ?? 0) - (b[i] ?? 0); if (delta) return delta; }
  return 0;
}

// Google Play requires a positive integer versionCode. Derive it from the
// canonical major.minor.build tuple instead of maintaining a second counter.
// Two decimal slots are reserved for minor and build so tuple ordering is
// preserved, while the resulting value remains below Play's 2.1B limit.
export function androidVersionCode(version) {
  if (!semver.test(version)) throw new Error('Use one x.y.z version for all platforms');
  const [major, minor, build] = version.split('.').map(Number);
  if (minor >= 100 || build >= 10000) throw new Error('Android versionCode requires minor < 100 and build < 10000');
  const code = major * 1_000_000 + minor * 10_000 + build;
  if (!Number.isSafeInteger(code) || code < 1 || code > 2_100_000_000) throw new Error('Derived Android versionCode is outside Google Play limits');
  return code;
}

// Plan every edit before writing any file: malformed input must not leave a
// half-bumped release. Store-required identifiers are deterministic functions
// of the version and cannot drift when the command is repeated.
export function planAppVersion(read, version) {
  if (!semver.test(version)) throw new Error('Use one x.y.z version for all platforms');
  const changes = new Map();
  for (const platform of ['windows', 'android', 'ios']) {
    const path = `apps/${platform}/package.json`, value = json(read(path));
    if (!semver.test(value.version)) throw new Error(`Invalid current ${platform} package version`);
    if (compare(version, value.version) < 0) throw new Error(`Refusing ${platform} version downgrade`);
    value.version = version; changes.set(path, formatJson(value));
  }
  const lock = json(read('package-lock.json'));
  for (const platform of ['windows', 'android', 'ios']) {
    if (!lock.packages?.[`apps/${platform}`]) throw new Error(`Missing ${platform} workspace lock entry`);
    lock.packages[`apps/${platform}`].version = version;
  }
  changes.set('package-lock.json', formatJson(lock));
  const tauri = json(read('apps/windows/src-tauri/tauri.conf.json'));
  tauri.version = version; changes.set('apps/windows/src-tauri/tauri.conf.json', formatJson(tauri));
  for (const [path, pattern] of [
    ['apps/windows/src-tauri/Cargo.toml', /(\[package\][\s\S]*?^version = ")[^"]+("[^\n]*)/gm],
    ['apps/windows/src-tauri/Cargo.lock', /(\[\[package\]\]\r?\nname = "qc-remote"\r?\nversion = ")[^"]+("[^\n]*)/g],
  ]) {
    const text = read(path); one(text, pattern, path);
    changes.set(path, text.replace(pattern, (_, before, after) => `${before}${version}${after}`));
  }
  let gradle = read(gradlePath);
  const name = one(gradle, /^(\s*versionName\s+")([^"]+)(")/gm, 'Android versionName');
  const code = one(gradle, /^(\s*versionCode\s+)(\d+)/gm, 'Android versionCode');
  const derivedCode = androidVersionCode(version);
  gradle = gradle.replace(name[0], `${name[1]}${version}${name[3]}`).replace(code[0], `${code[1]}${derivedCode}`);
  changes.set(gradlePath, gradle);
  const plist = read(plistPath);
  const marketing = one(plist, /(<key>CFBundleShortVersionString<\/key>\s*<string>)([^<]+)(<\/string>)/g, 'macOS broker version');
  const brokerBuild = one(plist, /(<key>CFBundleVersion<\/key>\s*<string>)([^<]+)(<\/string>)/g, 'macOS broker build version');
  changes.set(plistPath, plist.replace(marketing[0], `${marketing[1]}${version}${marketing[3]}`).replace(brokerBuild[0], `${brokerBuild[1]}${version}${brokerBuild[3]}`));
  const storeConfig = json(read(appStoreConfigPath));
  if (!storeConfig.bundle?.macOS) throw new Error('Missing macOS App Store bundle configuration');
  storeConfig.bundle.macOS.bundleVersion = version;
  changes.set(appStoreConfigPath, formatJson(storeConfig));
  let project = read(appProject);
  const configurations = appConfigs.map(id => appConfig(project, id));
  const oldVersions = configurations.map(c => one(c[2], /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1]);
  if (oldVersions.some(v => compare(version, v) < 0)) throw new Error('Refusing iOS marketing version downgrade');
  for (const c of configurations) {
    const body = c[2].replace(/MARKETING_VERSION = [^;]+;/, `MARKETING_VERSION = ${version};`).replace(/CURRENT_PROJECT_VERSION = [^;]+;/, `CURRENT_PROJECT_VERSION = ${version};`);
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
      console.log(`All QC Remote platforms use ${json(read('apps/windows/package.json')).version}.`);
    } else {
      const changes = planAppVersion(read, request);
      for (const [path, text] of changes) writeFileSync(resolve(root, path), text);
      console.log(`QC Remote ${request}: ${changes.size} version files synchronized; Android and Apple identifiers derived from the canonical version.`);
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
