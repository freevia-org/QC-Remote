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
//   * TypeScript is skipped only for markdown-only changes: tests read
//     docs/reference/*.json, the vite legal-assets plugin reads legal/**, and
//     qc-ui sources are what that job builds.
//
// The native globs are mirrored as `paths-ignore` in linux-broker.yml; the unit
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

export function isSafePath(file) {
  if (UNSAFE_EXCEPTIONS.includes(file)) return false;
  if (file.split('/').includes('node_modules')) return false;
  return SAFE_PATTERNS.some((pattern) => pattern.test(file));
}

export function isMarkdownOnlyPath(file) {
  if (MARKDOWN_EXCEPTIONS.includes(file)) return false;
  if (file.split('/').includes('node_modules')) return false;
  return MARKDOWN_PATTERNS.some((pattern) => pattern.test(file));
}

const RUN_EVERYTHING = { native: true, typescript: true, cloud: true };

/**
 * @returns {{ native: boolean, typescript: boolean, cloud: boolean, reason: string }}
 *   Every job runs unless its skip set covers every changed file.
 */
export function classify(files) {
  if (!Array.isArray(files) || files.length === 0) {
    return { ...RUN_EVERYTHING, reason: 'no changed files could be listed' };
  }
  const unsafe = files.filter((file) => !isSafePath(file));
  if (unsafe.length > 0) {
    const shown = unsafe.slice(0, 5).join(', ');
    const more = unsafe.length > 5 ? ', ...' : '';
    return { ...RUN_EVERYTHING, reason: `${unsafe.length} changed file(s) outside the safe set: ${shown}${more}` };
  }
  // Every changed file is in the safe set: no Rust, Android or relay input changed.
  const typescript = !files.every(isMarkdownOnlyPath);
  return {
    native: false,
    typescript,
    cloud: false,
    reason: `all ${files.length} changed file(s) are in the safe set${typescript ? '' : ' and are markdown only'}`,
  };
}

function git(args) {
  return execFileSync('git', args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
}

function changedFilesOfPullRequest() {
  const parents = git(['rev-list', '--parents', '-n', '1', 'HEAD']).trim().split(' ');
  // refs/pull/N/merge is a merge commit: first parent = base tip, so this diff
  // is exactly what merging the pull request would change.
  if (parents.length !== 3) throw new Error('HEAD is not a two-parent merge commit');
  return git(['diff', '--name-only', '--no-renames', '-z', 'HEAD^1', 'HEAD']).split('\0').filter(Boolean);
}

// A push to master is classified on everything it brought in (before..HEAD), so
// a merge that only touched legal or documentation files does not rebuild the
// native caches. The checkout is full-depth for pushes; a missing, all-zero or
// unreachable "before" falls back to running everything.
function changedFilesOfPush() {
  const before = process.env.QC_PUSH_BEFORE ?? '';
  if (!/^[0-9a-f]{40}$/.test(before) || /^0+$/.test(before)) throw new Error('no usable previous commit');
  return git(['diff', '--name-only', '--no-renames', '-z', before, 'HEAD']).split('\0').filter(Boolean);
}

function main() {
  const event = process.env.GITHUB_EVENT_NAME;
  let result;
  if (event !== 'pull_request' && event !== 'push') {
    result = { ...RUN_EVERYTHING, reason: `event ${event ?? '(none)'} is not a pull request or push` };
  } else {
    try {
      result = classify(event === 'push' ? changedFilesOfPush() : changedFilesOfPullRequest());
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
