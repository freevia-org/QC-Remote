// Security regression tests for the manually dispatched DCO workflow. The job
// reads only GitHub PR metadata and Git commit objects; it never executes PR code.

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const workflows = path.join(root, '.github', 'workflows');
const dco = readFileSync(path.join(workflows, 'dco.yml'), 'utf8');
const helper = readFileSync(path.join(root, 'scripts', 'verify-dco-dispatch.mjs'), 'utf8');

test('DCO runs only by workflow dispatch from protected master', () => {
  assert.match(dco, /^on:\r?\n  workflow_dispatch:/m);
  assert.match(dco, /^      pr_number:\r?\n/m);
  assert.doesNotMatch(dco, /^  (?:push|pull_request|pull_request_target|schedule):/m);
  assert.match(dco, /^    if: github\.ref == 'refs\/heads\/master'$/m);
  assert.match(dco, /^    name: DCO sign-off$/m);
});

test('DCO workflow uses read-only permissions and a pinned base checkout', () => {
  assert.match(dco, /^permissions:\r?\n  contents: read\r?\n  pull-requests: read$/m);
  assert.match(dco, /uses: actions\/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1/);
  assert.match(dco, /fetch-depth: 0/);
  assert.doesNotMatch(dco, /\bsecrets\./);
  assert.doesNotMatch(dco, /actions\/(?:cache|upload-artifact|download-artifact)/);
  assert.match(dco, /run: node scripts\/verify-dco-dispatch\.mjs/);
});

test('dispatch helper validates trusted PR metadata before fetching objects', () => {
  assert.match(helper, /repository !== 'freevia-org\/QC-Remote'/);
  assert.match(helper, /\^\[1-9\]\[0-9\]\*\$/);
  assert.match(helper, /pr\.state !== 'open'/);
  assert.match(helper, /pr\.base\?\.repo\?\.full_name !== repository/);
  assert.match(helper, /pr\.base\?\.ref !== 'master'/);
  assert.match(helper, /const baseSha = pr\.base\?\.sha/);
  assert.match(helper, /Authorization: `Bearer \$\{token\}`/);
  assert.match(helper, /https:\/\/api\.github\.com\/repos/);
  assert.match(helper, /\^\[0-9a-f\]\{40\}\$/);
  assert.match(helper, /new Set\(\['OWNER', 'MEMBER', 'COLLABORATOR'\]\)/);
  assert.match(helper, /pr\.user\?\.type === 'Bot'/);
  assert.match(helper, /refs\/pull\/\$\{number\}\/head:refs\/remotes\/pull\/\$\{number\}\/head/);
  assert.match(helper, /git', \['cat-file', '-e'/);
  assert.match(helper, /scripts\/verify-dco\.mjs/);
  assert.doesNotMatch(helper, /git', \['(?:checkout|switch|reset|merge|rebase)'/);
});

test('no other workflow uses pull_request_target', () => {
  for (const name of readdirSync(workflows).filter((entry) => /\.ya?ml$/.test(entry) && entry !== 'dco.yml')) {
    const text = readFileSync(path.join(workflows, name), 'utf8').replace(/^\s*#.*$/gm, '');
    assert.doesNotMatch(text, /^\s*pull_request_target:/m, `${name} must not use pull_request_target`);
  }
});
