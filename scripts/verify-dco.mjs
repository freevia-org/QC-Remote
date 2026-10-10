#!/usr/bin/env node
// Developer Certificate of Origin check for pull requests (see CONTRIBUTING.md).
//
// Every commit in <base>..<head> must carry a
// `Signed-off-by: Name <email>` trailer whose name AND email equal the commit's
// author. Commits already in the base branch are never examined, so the
// publisher's own earlier history needs no sign-off.
//
// This script exempts nobody. In particular:
//   * Bots and maintainers are not exempted here. Nothing in a commit (author
//     name, author email, committer) is authenticated: anyone can set
//     `git config user.email 'x[bot]@users.noreply.github.com'` or a
//     maintainer's name, so a commit-based exemption would let any contributor
//     skip the check. The exemption lives in .github/workflows/dco.yml instead,
//     where the `DCO sign-off` job is skipped when GitHub itself reports that
//     the pull request was opened by a Bot account
//     (github.event.pull_request.user.type) or that the author's association is
//     OWNER, MEMBER or COLLABORATOR
//     (github.event.pull_request.author_association). That workflow runs on
//     `pull_request_target`, so GitHub reads it from the base branch and a pull
//     request cannot edit, skip or widen it, and this script is run from the
//     base checkout, never from the pull request. Limits: anyone later added as
//     a collaborator becomes exempt; membership that GitHub does not report as
//     MEMBER is checked rather than exempted (the safe direction); and a human
//     commit pushed onto a maintainer's or bot's branch needs write access to
//     that branch, which is accepted.
//   * Merge commits are not exempted. A conflict resolution exists only inside
//     the merge commit and ends up in the squashed tree, so an unsigned merge
//     could carry unsigned changes. A contributor who merges the base into the
//     branch must sign that merge (`git merge --signoff`) or rebase instead;
//     GitHub's web "Update branch" merge commit carries no sign-off and fails
//     the check.
//
// What this can and cannot establish: it verifies that the trailer exists and
// names the author. It cannot verify that the name is a real one. CONTRIBUTING.md
// does not accept pseudonymous or anonymous sign-offs, and that part stays a
// matter for maintainer review.
//
// Usage: node scripts/verify-dco.mjs --base <rev> [--head <rev>] [--repo <path>]
// (QC_DCO_BASE / QC_DCO_HEAD / QC_DCO_REPO as fallbacks; an empty command-line
// value is an error, an empty environment value counts as unset).

import { execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const defaultRepositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// All `Signed-off-by: Name <email>` lines in a commit message, as
// { name, email }. The key is case-insensitive, as in git's own trailer rules;
// the value must be exactly "Name <email>" with a non-empty name and address.
export function parseSignoffs(message) {
  const signoffs = [];
  for (const line of message.split(/\r?\n/)) {
    const match = /^signed-off-by:[ \t]*(.*?)[ \t]*<([^<>\s]+)>[ \t]*$/i.exec(line);
    if (match && match[1].trim() !== '') {
      signoffs.push({ name: match[1].trim(), email: match[2] });
    }
  }
  return signoffs;
}

export function dcoErrors({ authorName, authorEmail, message }) {
  const signoffs = parseSignoffs(message);
  if (signoffs.length === 0) {
    return ['no Signed-off-by trailer (commit with `git commit -s`)'];
  }
  const name = authorName.trim();
  const email = authorEmail.trim().toLowerCase();
  if (signoffs.some((signoff) => signoff.name === name && signoff.email.toLowerCase() === email)) {
    return [];
  }
  return [`no Signed-off-by trailer matches the author "${name} <${authorEmail.trim()}>"`];
}

export function parseOptions(argv, env = {}) {
  const options = {
    base: env.QC_DCO_BASE || undefined,
    head: env.QC_DCO_HEAD || undefined,
    repo: env.QC_DCO_REPO || undefined,
  };
  const known = new Map([
    ['--base', 'base'],
    ['--head', 'head'],
    ['--repo', 'repo'],
  ]);
  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    const inline = token.indexOf('=');
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
  if (options.base === undefined) {
    throw new Error('--base is required (the DCO check only examines base..head)');
  }
  options.head ??= 'HEAD';
  return options;
}

function git(cwd, args) {
  return execFileSync('git', args, {
    cwd,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

// Returns { commits: [sha], errors: ["<sha>: ..."] }.
export function auditDco({ cwd = defaultRepositoryRoot, base, head = 'HEAD' }) {
  for (const rev of [base, head]) {
    try {
      git(cwd, ['rev-parse', '--verify', '--quiet', `${rev}^{commit}`]);
    } catch {
      throw new Error(`${rev} is not a commit in this repository (is the checkout shallow?)`);
    }
  }
  const unit = '\u001f';
  const record = '\u001e';
  const log = git(cwd, [
    'log',
    `--format=%H${unit}%an${unit}%ae${unit}%B${record}`,
    `${base}..${head}`,
  ]);
  const commits = [];
  const errors = [];
  for (const entry of log.split(record)) {
    if (entry.trim() === '') continue;
    const [sha, authorName, authorEmail, message] = entry.replace(/^\r?\n/, '').split(unit);
    commits.push(sha);
    for (const error of dcoErrors({ authorName, authorEmail, message })) {
      errors.push(`${sha}: ${error}`);
    }
  }
  return { commits, errors };
}

function main() {
  let options;
  let result;
  try {
    options = parseOptions(process.argv.slice(2), process.env);
    result = auditDco({
      cwd: options.repo ? path.resolve(options.repo) : defaultRepositoryRoot,
      base: options.base,
      head: options.head,
    });
  } catch (error) {
    console.error(`verify-dco: ${error.message}`);
    process.exit(2);
  }
  if (result.errors.length > 0) {
    console.error(
      `DCO sign-off check failed for ${result.errors.length} of ${result.commits.length} commit(s) in ${options.base}..${options.head}:\n- ` +
        `${result.errors.join('\n- ')}\n\nSign off with \`git commit -s\` (or \`git rebase --signoff <base>\` for a series). ` +
        'The sign-off must use your real name and the same name and email as the commit author; see CONTRIBUTING.md.',
    );
    process.exit(1);
  }
  console.log(
    `DCO sign-off present on ${result.commits.length} commit(s) in ${options.base}..${options.head}.`,
  );
}

if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  main();
}
