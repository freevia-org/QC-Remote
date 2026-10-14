#!/usr/bin/env node
// Decides whether a pull request needs the native (Rust / Android / Windows)
// CI jobs, for the `changes` job in .github/workflows/ci.yml.
//
// The rule is default-run: the native jobs are skipped only when EVERY changed
// file matches SAFE_GLOBS below. Anything else (workflows, Cargo files,
// lockfiles, contracts, scripts, services, Rust crates, src-tauri, skills,
// legal, qc-core/qc-theme/qc-client, app sources, ...) runs them. Any doubt
// (not a pull request, no merge commit, git failure, empty diff) also runs them.
//
// Why these globs are safe, from reading what the native jobs consume:
//   * packages/typescript/qc-ui source (.ts/.tsx/.css/.md): no Rust, Gradle or
//     script reads it (only comments name it). It reaches the Windows, Android,
//     iOS and macOS apps solely as input to their `vite build`, which the
//     `TypeScript packages and apps` job runs for all three web bundles on
//     every pull request. Non-source files (json, mjs, assets) stay unsafe.
//   * docs/** and root *.md: nothing in the native jobs reads them. Rust embeds
//     skills/** and packages/rust/**/docs, which are NOT listed.
// THIRD_PARTY-NOTICES.md is carved out of the root *.md glob: it is the legal
// inventory the notices job verifies and the native workflows list as a path.
//
// The same globs are mirrored as `paths-ignore` in linux-broker.yml; the unit
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
];

// Matched files that are nevertheless never safe.
export const UNSAFE_EXCEPTIONS = ['THIRD_PARTY-NOTICES.md'];

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

export function isSafePath(file) {
  if (UNSAFE_EXCEPTIONS.includes(file)) return false;
  if (file.split('/').includes('node_modules')) return false;
  return SAFE_PATTERNS.some((pattern) => pattern.test(file));
}

/** @returns {{ native: boolean, reason: string }} */
export function classify(files) {
  if (!Array.isArray(files) || files.length === 0) {
    return { native: true, reason: 'no changed files could be listed' };
  }
  const unsafe = files.filter((file) => !isSafePath(file));
  if (unsafe.length > 0) {
    const shown = unsafe.slice(0, 5).join(', ');
    const more = unsafe.length > 5 ? ', ...' : '';
    return { native: true, reason: `${unsafe.length} changed file(s) outside the safe set: ${shown}${more}` };
  }
  return { native: false, reason: `all ${files.length} changed file(s) are in the safe set` };
}

function changedFiles() {
  const parents = execFileSync('git', ['rev-list', '--parents', '-n', '1', 'HEAD'], { encoding: 'utf8' })
    .trim()
    .split(' ');
  // refs/pull/N/merge is a merge commit: first parent = base tip, so this diff
  // is exactly what merging the pull request would change.
  if (parents.length !== 3) throw new Error('HEAD is not a two-parent merge commit');
  const out = execFileSync('git', ['diff', '--name-only', '--no-renames', '-z', 'HEAD^1', 'HEAD'], {
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  return out.split('\0').filter(Boolean);
}

function main() {
  let result;
  if (process.env.GITHUB_EVENT_NAME !== 'pull_request') {
    result = { native: true, reason: `event ${process.env.GITHUB_EVENT_NAME ?? '(none)'} is not a pull request` };
  } else {
    try {
      result = classify(changedFiles());
    } catch (error) {
      result = { native: true, reason: `could not determine the changed files (${error.message})` };
    }
  }
  console.log(`native=${result.native}: ${result.reason}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `native=${result.native}\n`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
