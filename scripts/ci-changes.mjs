#!/usr/bin/env node
// Decides which CI jobs a pull request needs, for the `changes` job in
// .github/workflows/ci.yml. Three independent answers are emitted:
//
//   native      the Rust / Windows app / Android jobs (and, mirrored as
//               paths-ignore, the Linux, macOS and iOS workflows)
//   typescript  the TypeScript packages and apps job
//   cloud       the Cloud relay dry-run job
//
// The rule is default-run: a job is skipped only when EVERY changed file matches
// that job's skip set below. Anything else (workflows, Cargo files, lockfiles,
// contracts, scripts, services, Rust crates, src-tauri, skills, qc-core,
// qc-theme, qc-client, app sources, ...) runs it. Any doubt (not a pull request,
// no merge commit, git failure, empty diff) also runs everything.
//
// Required checks never deadlock on this: a job skipped by a job-level `if:`
// reports "Skipped", which branch protection accepts, and the filters that sit
// on workflows nobody requires (linux-broker, macos-build, ios) are
// workflow-level paths. The `CI result` aggregate job in ci.yml is the single
// context to require if the owner wants one.
//
// Why these globs are safe, from reading what the jobs consume:
//   * packages/typescript/qc-ui source (.ts/.tsx/.css/.md): no Rust, Gradle or
//     script reads it (only comments name it). It reaches the Windows, Android,
//     iOS and macOS apps solely as input to their `vite build`, which the
//     `TypeScript packages and apps` job runs for all three web bundles on
//     every pull request that is not markdown only. Non-source files (json,
//     mjs, assets) stay unsafe.
//   * docs/** and root *.md: nothing in the native jobs reads them. Rust embeds
//     skills/** and packages/rust/**/docs, which are NOT listed.
//   * Legal records (legal/**, LICENSE*, LICENSES/**, THIRD_PARTY-NOTICES.md).
//     The Rust code only `include_bytes!`es them and tauri.conf.json only lists
//     them as bundle resources, so their text cannot change a compile; the one
//     content-dependent Rust check (every relative link in a bundled skill file
//     names a bundled file) is mirrored by scripts/verify-license-carveout.mjs,
//     which the always-run `Third-party notices drift check` job executes next
//     to the inventory/notices verifier. Changing WHICH legal file is bundled
//     means editing device_knowledge_skill.rs, tauri.conf.json or
//     verify-license-carveout.mjs, none of which is a safe path.
//   * The device-knowledge skill carries byte copies of the same legal files
//     (skills/qc-device-knowledge/{LICENSE*,LICENSES,legal,THIRD_PARTY-NOTICES.md});
//     the Rust shell embeds them as bytes too and verify-license-carveout.mjs
//     keeps them identical to the originals. SKILL.md and the rest of skills/**
//     stay unsafe: a Rust test asserts on their content.
//   * Neither the Cloud relay (services/qc-remote-cloud plus qc-theme) nor the
//     Rust jobs read any of the above, so the Cloud relay is skipped for them.
//   * The Cloud relay Worker sources (CLOUD_ONLY_GLOBS below) are native
//     independent: no Rust crate, Gradle file, Tauri config, script run by a
//     native job, or iOS/macOS/Linux workflow reads, compiles, embeds or tests
//     anything under services/qc-remote-cloud. Its only consumers are the
//     `TypeScript packages and apps` job (qc-core's mcp-adapter test imports
//     action-projection.ts and reads index.ts; contracts:check loads
//     device-deadlines.ts) and the `Cloud relay` job, both of which still run
//     for such a diff. The rest of the directory is NOT listed: package.json is
//     an npm workspace manifest that `npm ci` in the Android job validates
//     against the root lockfile, so it counts only when the diff provably
//     changes nothing but non-lifecycle `scripts` entries
//     (isCloudScriptsOnlyEdit); a Cargo.toml, .rs file or any file type not
//     listed keeps every job on.
//   * TypeScript is skipped only for markdown-only changes: tests read
//     docs/reference/*.json, the vite legal-assets plugin reads legal/**, and
//     qc-ui sources are what that job builds.
//
// NATIVE_SKIP_GLOBS are mirrored as `paths-ignore` in linux-broker.yml; the unit
// test pins that the two lists are identical.
import { execFileSync } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

export const SAFE_GLOBS = [
  'packages/typescript/qc-ui/**/*.ts',
  'packages/typescript/qc-ui/**/*.tsx',
  'packages/typescript/qc-ui/**/*.css',
  'packages/typescript/qc-ui/**/*.md',
  'docs/**',
  '*.md',
  'legal/**',
  'LICENSE',
  'LICENSE-MIT',
  'LICENSE-APACHE',
  'LICENSES/**',
  'skills/qc-device-knowledge/LICENSE',
  'skills/qc-device-knowledge/LICENSE-MIT',
  'skills/qc-device-knowledge/LICENSE-APACHE',
  'skills/qc-device-knowledge/LICENSES/**',
  'skills/qc-device-knowledge/legal/**',
  'skills/qc-device-knowledge/THIRD_PARTY-NOTICES.md',
];

// services/qc-remote-cloud files no native job can observe (see the header).
// Only these file kinds, in these directories: everything else under the
// service stays default-run.
export const CLOUD_ONLY_GLOBS = [
  'services/qc-remote-cloud/src/**/*.ts',
  'services/qc-remote-cloud/tests/**/*.ts',
  'services/qc-remote-cloud/scripts/**/*.mjs',
  'services/qc-remote-cloud/wrangler.jsonc',
  'services/qc-remote-cloud/wrangler.*.jsonc',
  'services/qc-remote-cloud/worker-configuration.d.ts',
  'services/qc-remote-cloud/tsconfig.json',
  'services/qc-remote-cloud/tsconfig.*.json',
  'services/qc-remote-cloud/vitest.*.config.ts',
  'services/qc-remote-cloud/README.md',
];

// What linux-broker.yml mirrors as `pull_request.paths-ignore`: a path none of
// the native jobs and workflows can observe.
export const NATIVE_SKIP_GLOBS = [...SAFE_GLOBS, ...CLOUD_ONLY_GLOBS];

export const CLOUD_PACKAGE_JSON = 'services/qc-remote-cloud/package.json';
// npm runs these around install/pack/publish (and `npm run build --workspaces`
// runs build): editing one is not a "scripts only" edit.
const LIFECYCLE_SCRIPT = /^(?:pre|post)?(?:install|prepare|prepublish|prepublishOnly|publish|pack|build|version|dependencies|uninstall|shrinkwrap)$/;

/**
 * True only when before/after are both valid JSON objects that are identical
 * except for `scripts` entries, and no added, removed or changed script is an
 * npm lifecycle script. Anything else (parse failure, new or deleted file,
 * dependency, version, name, overrides, workspaces, files edits) is false.
 */
export function isCloudScriptsOnlyEdit(beforeText, afterText) {
  let before;
  let after;
  try {
    before = JSON.parse(beforeText);
    after = JSON.parse(afterText);
  } catch {
    return false;
  }
  const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
  if (!isObject(before) || !isObject(after)) return false;
  const { scripts: scriptsBefore = {}, ...restBefore } = before;
  const { scripts: scriptsAfter = {}, ...restAfter } = after;
  if (!isObject(scriptsBefore) || !isObject(scriptsAfter)) return false;
  if (JSON.stringify(restBefore) !== JSON.stringify(restAfter)) return false;
  const names = new Set([...Object.keys(scriptsBefore), ...Object.keys(scriptsAfter)]);
  for (const name of names) {
    if (scriptsBefore[name] !== scriptsAfter[name] && LIFECYCLE_SCRIPT.test(name)) return false;
  }
  return true;
}

// Matched files that are nevertheless never safe.
export const UNSAFE_EXCEPTIONS = [];

// The TypeScript job may be skipped only when every changed file is markdown
// that no build or test reads as code. THIRD_PARTY-NOTICES.md is carved out: the
// vite legal-assets plugin and the legal tests consume it.
export const MARKDOWN_ONLY_GLOBS = ['*.md', 'docs/**/*.md'];
export const MARKDOWN_EXCEPTIONS = ['THIRD_PARTY-NOTICES.md'];

export function globToRegExp(glob) {
  let source = '';
  for (let i = 0; i < glob.length; i += 1) {
    const char = glob[i];
    if (char === '*' && glob[i + 1] === '*') {
      if (glob[i + 2] === '/') {
        source += '(?:.*/)?';
        i += 2;
      } else {
        source += '.*';
        i += 1;
      }
    } else if (char === '*') {
      source += '[^/]*';
    } else {
      source += char.replace(/[.+?^${}()|[\]\\]/g, '\\$&');
    }
  }
  return new RegExp(`^${source}$`);
}

const SAFE_PATTERNS = SAFE_GLOBS.map(globToRegExp);
const MARKDOWN_PATTERNS = MARKDOWN_ONLY_GLOBS.map(globToRegExp);
const CLOUD_ONLY_PATTERNS = CLOUD_ONLY_GLOBS.map(globToRegExp);

export function isSafePath(file) {
  if (UNSAFE_EXCEPTIONS.includes(file)) return false;
  if (file.split('/').includes('node_modules')) return false;
  return SAFE_PATTERNS.some((pattern) => pattern.test(file));
}

export function isCloudOnlyPath(file) {
  if (file.split('/').includes('node_modules')) return false;
  return CLOUD_ONLY_PATTERNS.some((pattern) => pattern.test(file));
}

export function isMarkdownOnlyPath(file) {
  if (MARKDOWN_EXCEPTIONS.includes(file)) return false;
  if (file.split('/').includes('node_modules')) return false;
  return MARKDOWN_PATTERNS.some((pattern) => pattern.test(file));
}

const RUN_EVERYTHING = { native: true, typescript: true, cloud: true };

/**
 * @param {string[]} files
 * @param {{ cloudPackageJsonEdit?: (file: string) => boolean }} [context]
 *   `cloudPackageJsonEdit` answers whether the diff of services/qc-remote-cloud/package.json
 *   is a scripts-only edit; without it that file is never skippable (fail closed).
 * @returns {{ native: boolean, typescript: boolean, cloud: boolean, reason: string }}
 *   Every job runs unless its skip set covers every changed file.
 */
export function classify(files, context = {}) {
  if (!Array.isArray(files) || files.length === 0) {
    return { ...RUN_EVERYTHING, reason: 'no changed files could be listed' };
  }
  const nativeFree = (file) =>
    isSafePath(file) ||
    isCloudOnlyPath(file) ||
    (file === CLOUD_PACKAGE_JSON && context.cloudPackageJsonEdit?.(file) === true);
  const unsafe = files.filter((file) => !nativeFree(file));
  if (unsafe.length > 0) {
    const shown = unsafe.slice(0, 5).join(', ');
    const more = unsafe.length > 5 ? ', ...' : '';
    return { ...RUN_EVERYTHING, reason: `${unsafe.length} changed file(s) outside the safe set: ${shown}${more}` };
  }
  // Every changed file is native independent: no Rust, Android, Tauri or iOS
  // input changed. The relay still runs when any file is not a plain safe path.
  const typescript = !files.every(isMarkdownOnlyPath);
  const cloud = !files.every(isSafePath);
  return {
    native: false,
    typescript,
    cloud,
    reason: `all ${files.length} changed file(s) are native independent${cloud ? ' (relay sources: relay and TypeScript jobs still run)' : ''}${typescript ? '' : ' and are markdown only'}`,
  };
}

function git(args) {
  return execFileSync('git', args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
}

function cloudPackageJsonEditSince(base) {
  return () => {
    try {
      return isCloudScriptsOnlyEdit(git(['show', `${base}:${CLOUD_PACKAGE_JSON}`]), git(['show', `HEAD:${CLOUD_PACKAGE_JSON}`]));
    } catch {
      return false;
    }
  };
}

function changedFilesOfPullRequest() {
  const parents = git(['rev-list', '--parents', '-n', '1', 'HEAD']).trim().split(' ');
  // refs/pull/N/merge is a merge commit: first parent = base tip, so this diff
  // is exactly what merging the pull request would change.
  if (parents.length !== 3) throw new Error('HEAD is not a two-parent merge commit');
  return {
    files: git(['diff', '--name-only', '--no-renames', '-z', 'HEAD^1', 'HEAD']).split('\0').filter(Boolean),
    base: 'HEAD^1',
  };
}

// A push to master is classified on everything it brought in (before..HEAD), so
// a merge that only touched legal or documentation files does not rebuild the
// native caches. The checkout is full-depth for pushes; a missing, all-zero or
// unreachable "before" falls back to running everything.
function changedFilesOfPush() {
  const before = process.env.QC_PUSH_BEFORE ?? '';
  if (!/^[0-9a-f]{40}$/.test(before) || /^0+$/.test(before)) throw new Error('no usable previous commit');
  return {
    files: git(['diff', '--name-only', '--no-renames', '-z', before, 'HEAD']).split('\0').filter(Boolean),
    base: before,
  };
}

function main() {
  const event = process.env.GITHUB_EVENT_NAME;
  let result;
  if (event !== 'pull_request' && event !== 'push') {
    result = { ...RUN_EVERYTHING, reason: `event ${event ?? '(none)'} is not a pull request or push` };
  } else {
    try {
      const { files, base } = event === 'push' ? changedFilesOfPush() : changedFilesOfPullRequest();
      result = classify(files, { cloudPackageJsonEdit: cloudPackageJsonEditSince(base) });
    } catch (error) {
      result = { ...RUN_EVERYTHING, reason: `could not determine the changed files (${error.message})` };
    }
  }
  console.log(`native=${result.native} typescript=${result.typescript} cloud=${result.cloud}: ${result.reason}`);
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(
      process.env.GITHUB_OUTPUT,
      `native=${result.native}
typescript=${result.typescript}
cloud=${result.cloud}
`,
    );
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
