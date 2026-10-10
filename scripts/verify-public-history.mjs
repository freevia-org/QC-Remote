#!/usr/bin/env node
// Checks reachable commits, not only the checked-out tree. Public history must
// contain only public paths and privacy-preserving Git identities.
//
// Two modes:
//
//   whole history (default)   every commit reachable from any ref. This is what
//                             a push to master (and a local run) uses: the
//                             history that is, or is about to be, published.
//
//   range  (--base <rev>)     only the commits in <base>..<head> (head defaults
//                             to HEAD). Pull requests use this so a contributor
//                             is judged on what the PR adds, not on every
//                             branch the checkout happens to carry.
//
// The path boundary is a hard failure in both modes. The identity rules (noreply
// author/committer, no Co-authored-by trailer) are a hard failure by default; in
// range mode they can be downgraded to warnings with --identity advisory. That
// is for pull requests from forks: the Developer Certificate of Origin that
// CONTRIBUTING.md asks for carries a real-name sign-off, and an outside
// a maintainer must publish a privacy-preserving, signed-off commit instead of
// publishing those identities as they stand. The commit that lands on master is
// checked in whole-history mode by the push run, which is the point where the
// identity rules are enforced for good.
//
// Usage:
//   node scripts/verify-public-history.mjs
//   node scripts/verify-public-history.mjs --base <rev> [--head <rev>]
//        [--identity strict|advisory] [--repo <path>]
// Environment fallbacks: QC_HISTORY_BASE, QC_HISTORY_HEAD, QC_HISTORY_IDENTITY,
// QC_HISTORY_REPO (an empty value counts as unset; an empty command-line value
// is an error, so a CI expression that expands to nothing cannot silently fall
// back to a different mode).

import { execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const defaultRepositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function git(cwd, args) {
  return execFileSync('git', args, {
    cwd,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

export const forbiddenPaths = [
  /(^|\/)\.claude(\/|$)/i,
  /(^|\/)\.codex(\/|$)/i,
  /(^|\/)\.vendor(\/|$)/i,
  /(^|\/)contracts\/private(\/|$)/i,
  /(^|\/)references\/private(\/|$)/i,
  /(^|\/)artifacts\/cortex-hid(\/|$)/i,
  /(^|\/)\.env(?:\.|$)/i,
  /\.(?:jks|keystore|p12|pfx|pem|key)$/i,
];

// Addresses, other than GitHub noreply ones, that may appear as a commit author
// or committer. Each entry is an organisation contact address that is public by
// design (the repository is published from the Freevia organisation account
// under it), so it discloses nothing a personal address would. Matching is an
// exact, case-insensitive comparison of the whole address: a different domain
// that merely contains or extends it ("info@freevia.org.evil.com") is rejected.
// Add an entry here only for an address that is already public.
export const allowedPublisherEmails = ['info@freevia.org'];

// GitHub's first squash merge after public-history enforcement was enabled
// appended a duplicate, privacy-preserving Co-authored-by trailer even though
// the reviewed source commit did not contain one. That commit was published
// before squash merges were disabled at the repository level. Keep the strict
// rule for every other commit and document this single immutable exception so
// the whole-history audit can continue to protect all later history.
export const publishedIdentityExceptions = new Map([
  [
    '2badc68ea4d30f53fc98ae9e000c70df109d0970',
    new Set(['commit message contains a Co-authored-by trailer']),
  ],
]);

export function isPublishedIdentityException(commit, finding) {
  return publishedIdentityExceptions.get(commit)?.has(finding) ?? false;
}

export function isPrivacyPreservingIdentity(email) {
  const normalized = email.trim().toLowerCase();
  return (
    normalized === 'noreply' ||
    normalized.endsWith('@users.noreply.github.com') ||
    allowedPublisherEmails.includes(normalized)
  );
}

// GitHub itself commits merges, squashes and web edits made on github.com as
// "GitHub <noreply@github.com>" (the web-flow identity). That identity names no
// person, so it is acceptable as a committer. It is never acceptable as an
// author: the author is always the human (or bot) whose change it is.
export function isGitHubWebFlowCommitter(name, email) {
  return name.trim() === 'GitHub' && email.trim().toLowerCase() === 'noreply@github.com';
}

export function identityErrors({ authorEmail, committerName, committerEmail }) {
  const errors = [];
  if (!isPrivacyPreservingIdentity(authorEmail)) {
    errors.push('author email is not a noreply identity');
  }
  if (
    !isPrivacyPreservingIdentity(committerEmail) &&
    !isGitHubWebFlowCommitter(committerName, committerEmail)
  ) {
    errors.push('committer email is not a noreply identity');
  }
  return errors;
}

export function parseOptions(argv, env = {}) {
  const options = { base: undefined, head: undefined, identity: 'strict', repo: undefined };
  const fromEnv = (name) => (env[name] ? env[name] : undefined);
  options.base = fromEnv('QC_HISTORY_BASE');
  options.head = fromEnv('QC_HISTORY_HEAD');
  options.identity = fromEnv('QC_HISTORY_IDENTITY') ?? 'strict';
  options.repo = fromEnv('QC_HISTORY_REPO');

  const known = new Map([
    ['--base', 'base'],
    ['--head', 'head'],
    ['--identity', 'identity'],
    ['--repo', 'repo'],
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    const inline = token.includes('=') ? token.indexOf('=') : -1;
    const flag = inline === -1 ? token : token.slice(0, inline);
    const key = known.get(flag);
    if (!key) {
      throw new Error(`unknown argument: ${token}`);
    }
    let value;
    if (inline !== -1) {
      value = token.slice(inline + 1);
    } else {
      index += 1;
      if (index >= argv.length) {
        throw new Error(`${flag} needs a value`);
      }
      value = argv[index];
    }
    if (value === '') {
      throw new Error(`${flag} must not be empty`);
    }
    options[key] = value;
  }

  if (options.identity !== 'strict' && options.identity !== 'advisory') {
    throw new Error(`--identity must be "strict" or "advisory", not "${options.identity}"`);
  }
  if (options.head !== undefined && options.base === undefined) {
    throw new Error('--head only applies together with --base');
  }
  if (options.identity === 'advisory' && options.base === undefined) {
    // Whole-history mode is the published history: never let it be relaxed.
    throw new Error('--identity advisory only applies to range mode (--base)');
  }
  return options;
}

// Returns { mode, commits, errors, warnings } for the repository at `cwd`.
export function auditHistory({ cwd = defaultRepositoryRoot, base, head = 'HEAD', identity = 'strict' } = {}) {
  let commits;
  let mode;
  if (base === undefined) {
    mode = 'whole history';
    commits = git(cwd, ['rev-list', '--all']);
  } else {
    mode = `range ${base}..${head}`;
    for (const rev of [base, head]) {
      try {
        git(cwd, ['rev-parse', '--verify', '--quiet', `${rev}^{commit}`]);
      } catch {
        throw new Error(`${rev} is not a commit in this repository (is the checkout shallow?)`);
      }
    }
    commits = git(cwd, ['rev-list', `${base}..${head}`]);
  }
  commits = commits.trim().split(/\r?\n/).filter(Boolean);

  const errors = [];
  const warnings = [];
  const identityFindings = identity === 'advisory' ? warnings : errors;

  for (const commit of commits) {
    const metadata = git(cwd, ['show', '-s', '--format=%ae%n%cn%n%ce%n%B', commit]);
    const [authorEmail, committerName, committerEmail, ...messageLines] = metadata.split(/\r?\n/);
    for (const error of identityErrors({ authorEmail, committerName, committerEmail })) {
      if (!isPublishedIdentityException(commit, error)) identityFindings.push(`${commit}: ${error}`);
    }
    if (messageLines.some((line) => /^co-authored-by:/i.test(line.trim()))) {
      const finding = 'commit message contains a Co-authored-by trailer';
      if (!isPublishedIdentityException(commit, finding)) identityFindings.push(`${commit}: ${finding}`);
    }

    const paths = git(cwd, ['ls-tree', '-r', '--name-only', commit]).split(/\r?\n/).filter(Boolean);
    for (const trackedPath of paths) {
      if (forbiddenPaths.some((pattern) => pattern.test(trackedPath))) {
        errors.push(`${commit}: forbidden public path ${trackedPath}`);
      }
    }
  }

  return { mode, commits, errors, warnings };
}

function main() {
  let options;
  try {
    options = parseOptions(process.argv.slice(2), process.env);
  } catch (error) {
    console.error(`verify-public-history: ${error.message}`);
    process.exit(2);
  }

  let result;
  try {
    result = auditHistory({
      cwd: options.repo ? path.resolve(options.repo) : defaultRepositoryRoot,
      base: options.base,
      head: options.head,
      identity: options.identity,
    });
  } catch (error) {
    console.error(`verify-public-history: ${error.message}`);
    process.exit(2);
  }

  for (const warning of result.warnings) {
    // Advisory findings never fail the run; surface them as GitHub annotations too.
    console.warn(`warning: ${warning}`);
    if (process.env.GITHUB_ACTIONS) {
      console.warn(`::warning title=Public history identity (advisory)::${warning}`);
    }
  }

  if (result.errors.length > 0) {
    console.error(`Public-history boundary failed (${result.mode}):\n- ${result.errors.join('\n- ')}`);
    process.exit(1);
  }

  if (options.base === undefined) {
    console.log(`Public history is clean across ${result.commits.length} reachable commit(s).`);
  } else {
    console.log(
      `Public-history boundary is clean across ${result.commits.length} commit(s) in ${result.mode}` +
        (result.warnings.length > 0 ? ` (${result.warnings.length} advisory identity warning(s))` : '') +
        '.',
    );
  }
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  main();
}
