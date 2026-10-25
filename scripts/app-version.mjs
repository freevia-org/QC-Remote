// One public marketing version for every QC Remote application. Library and
// protocol versions, Android versionCode and Apple build numbers are independent.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const semver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const appProject = 'apps/ios/ios/App/App.xcodeproj/project.pbxproj';
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
  check('Android versionName', one(read(gradlePath), /^\s*versionName\s+"([^"]+)"/gm, 'Android versionName')[1]);
  check('macOS broker', one(read(plistPath), /<key>CFBundleShortVersionString<\/key>\s*<string>([^<]+)<\/string>/g, 'macOS broker version')[1]);
  for (const id of appConfigs) check(`iOS App ${id}`, one(appConfig(read(appProject), id)[2], /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1]);
  return problems;
}

function compare(left, right) {
  const a = left.split('.').map(Number), b = right.split('.').map(Number);
  for (let i = 0; i < 3; i++) { const delta = (a[i] ?? 0) - (b[i] ?? 0); if (delta) return delta; }
  return 0;
}

// Plan every edit before writing any file: malformed input must not leave a
// half-bumped release. A repeated sync never consumes another store build number.
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
  const nextCode = Number(code[2]) + (name[2] === version ? 0 : 1);
  if (!Number.isSafeInteger(nextCode) || nextCode < 1 || nextCode > 2100000000) throw new Error('Invalid Android versionCode');
  gradle = gradle.replace(name[0], `${name[1]}${version}${name[3]}`).replace(code[0], `${code[1]}${nextCode}`);
  changes.set(gradlePath, gradle);
  const plist = read(plistPath);
  const marketing = one(plist, /(<key>CFBundleShortVersionString<\/key>\s*<string>)([^<]+)(<\/string>)/g, 'macOS broker version');
  changes.set(plistPath, plist.replace(marketing[0], `${marketing[1]}${version}${marketing[3]}`));
  let project = read(appProject);
  const configurations = appConfigs.map(id => appConfig(project, id));
  const oldBuilds = configurations.map(c => Number(one(c[2], /CURRENT_PROJECT_VERSION = (\d+);/g, 'iOS build number')[1]));
  if (oldBuilds[0] !== oldBuilds[1] || !Number.isSafeInteger(oldBuilds[0]) || oldBuilds[0] < 1) throw new Error('iOS App build numbers disagree');
  const oldVersions = configurations.map(c => one(c[2], /MARKETING_VERSION = ([^;]+);/g, 'iOS marketing version')[1]);
  if (oldVersions.some(v => compare(version, v) < 0)) throw new Error('Refusing iOS marketing version downgrade');
  const nextBuild = oldBuilds[0] + (oldVersions.every(v => v === version) ? 0 : 1);
  if (!Number.isSafeInteger(nextBuild)) throw new Error('Invalid next iOS App build number');
  for (const c of configurations) {
    const body = c[2].replace(/MARKETING_VERSION = [^;]+;/, `MARKETING_VERSION = ${version};`).replace(/CURRENT_PROJECT_VERSION = \d+;/, `CURRENT_PROJECT_VERSION = ${nextBuild};`);
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
      console.log(`QC Remote ${request}: ${changes.size} version files synchronized; store build numbers preserved or advanced.`);
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
