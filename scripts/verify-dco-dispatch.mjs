#!/usr/bin/env node

// Maintainer-dispatched DCO validation. The workflow runs this file from the
// protected master branch; pull-request data is obtained through the GitHub API
// and fetched as Git objects only. No pull-request files are checked out or run.

import { execFileSync } from 'node:child_process';

const repository = process.env.GITHUB_REPOSITORY;
const token = process.env.GITHUB_TOKEN;
const number = process.env.QC_PR_NUMBER;

function fail(message) {
  console.error(`DCO check failed: ${message}`);
  process.exit(1);
}

if (repository !== 'freevia-org/QC-Remote') fail('unexpected repository');
if (!/^[1-9][0-9]*$/.test(number ?? '')) fail('invalid pull request number');
if (!token) fail('GitHub token is unavailable');

const response = await fetch(`https://api.github.com/repos/${repository}/pulls/${number}`, {
  headers: {
    Accept: 'application/vnd.github+json',
    Authorization: `Bearer ${token}`,
    'X-GitHub-Api-Version': '2022-11-28',
  },
});
if (!response.ok) fail(`GitHub returned HTTP ${response.status} for PR ${number}`);

const pr = await response.json();
if (pr.number !== Number(number) || pr.state !== 'open') fail('PR is missing or closed');
if (pr.base?.repo?.full_name !== repository || pr.base?.ref !== 'master') {
  fail('PR must target master in this repository');
}
const headSha = pr.head?.sha;
if (!/^[0-9a-f]{40}$/.test(headSha ?? '')) fail('invalid pull request head SHA');
const baseSha = pr.base?.sha;
if (!/^[0-9a-f]{40}$/.test(baseSha ?? '')) fail('invalid pull request base SHA');

const trusted = new Set(['OWNER', 'MEMBER', 'COLLABORATOR']);
if (pr.user?.type === 'Bot' || trusted.has(pr.author_association)) {
  console.log(`DCO check skipped for trusted PR author (${pr.author_association ?? pr.user?.type}).`);
  process.exit(0);
}

// The pull-request number is validated above. Fetch the head as an object ref,
// confirm the exact API SHA is present, and pass only SHA values to the checker.
execFileSync('git', [
  'fetch', '--no-tags', '--no-recurse-submodules', 'origin',
  `+refs/pull/${number}/head:refs/remotes/pull/${number}/head`,
], { stdio: 'inherit' });
execFileSync('git', ['cat-file', '-e', `${headSha}^{commit}`], { stdio: 'inherit' });
execFileSync('node', ['scripts/verify-dco.mjs', '--base', baseSha, '--head', headSha], {
  stdio: 'inherit',
});
