import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import * as checker from './verify-dco.mjs';
import { auditDco, dcoErrors, parseOptions, parseSignoffs } from './verify-dco.mjs';

const author = { authorName: 'Ada Lovelace', authorEmail: 'ada@example.com' };

// ---- trailer parsing --------------------------------------------------------

test('a Signed-off-by line is parsed into name and email', () => {
  assert.deepEqual(parseSignoffs('Fix a thing\n\nBody.\n\nSigned-off-by: Ada Lovelace <ada@example.com>\n'), [
    { name: 'Ada Lovelace', email: 'ada@example.com' },
  ]);
});

test('the key is case-insensitive, CRLF messages work, several sign-offs are all returned', () => {
  assert.deepEqual(
    parseSignoffs('x\r\n\r\nsigned-off-by: A B <a@x.org>\r\nSIGNED-OFF-BY:C D<c@x.org>\r\n'),
    [
      { name: 'A B', email: 'a@x.org' },
      { name: 'C D', email: 'c@x.org' },
    ],
  );
});

test('lines that only look like a sign-off are not parsed', () => {
  for (const line of [
    'Signed-off-by: <ada@example.com>',
    'Signed-off-by: Ada Lovelace',
    'Signed-off-by: Ada Lovelace <>',
    'Signed-off-by: Ada Lovelace <ada@example.com> trailing words',
    '  not at the start. Signed-off-by: Ada Lovelace <ada@example.com>',
    'Signed-off: Ada Lovelace <ada@example.com>',
    'Co-authored-by: Ada Lovelace <ada@example.com>',
  ]) {
    assert.deepEqual(parseSignoffs(`subject\n\n${line}\n`), [], line);
  }
});

// ---- the rule ----------------------------------------------------------------

test('a sign-off naming the author passes; the email is compared case-insensitively', () => {
  assert.deepEqual(dcoErrors({ ...author, message: 's\n\nSigned-off-by: Ada Lovelace <ada@example.com>' }), []);
  assert.deepEqual(dcoErrors({ ...author, message: 's\n\nSigned-off-by: Ada Lovelace <ADA@Example.com>' }), []);
});

test('a commit without any sign-off fails', () => {
  assert.deepEqual(dcoErrors({ ...author, message: 'just a subject' }), [
    'no Signed-off-by trailer (commit with `git commit -s`)',
  ]);
});

test('a sign-off for someone else, or with another name or address, fails', () => {
  for (const trailer of [
    'Signed-off-by: Grace Hopper <grace@example.com>',
    'Signed-off-by: Ada Lovelace <other@example.com>',
    'Signed-off-by: Ada L. <ada@example.com>',
    'Signed-off-by: ada lovelace <ada@example.com>',
  ]) {
    assert.deepEqual(
      dcoErrors({ ...author, message: `s\n\n${trailer}` }),
      ['no Signed-off-by trailer matches the author "Ada Lovelace <ada@example.com>"'],
      trailer,
    );
  }
});

test('one matching sign-off among several is enough', () => {
  assert.deepEqual(
    dcoErrors({
      ...author,
      message: 's\n\nSigned-off-by: Grace Hopper <grace@example.com>\nSigned-off-by: Ada Lovelace <ada@example.com>',
    }),
    [],
  );
});

test('a bot-looking author address earns no exemption: it is user-controlled', () => {
  for (const authorEmail of [
    'x[bot]@users.noreply.github.com',
    '49699333+dependabot[bot]@users.noreply.github.com',
  ]) {
    assert.deepEqual(dcoErrors({ authorName: 'dependabot[bot]', authorEmail, message: 'Bump x' }), [
      'no Signed-off-by trailer (commit with `git commit -s`)',
    ]);
    assert.deepEqual(
      dcoErrors({
        authorName: 'dependabot[bot]',
        authorEmail,
        message: `Bump x

Signed-off-by: dependabot[bot] <${authorEmail}>`,
      }),
      [],
    );
  }
  assert.equal(checker.isBotAuthor, undefined, 'no email-based bot test is exported');
});

// ---- options -----------------------------------------------------------------

test('--base is required; empty values are errors; env fills in unset flags', () => {
  assert.throws(() => parseOptions([], {}), /--base is required/);
  assert.throws(() => parseOptions(['--base', ''], {}), /must not be empty/);
  assert.throws(() => parseOptions(['--nope'], {}), /unknown argument/);
  assert.throws(() => parseOptions(['--head'], { QC_DCO_BASE: 'a' }), /needs a value/);
  assert.deepEqual(parseOptions([], { QC_DCO_BASE: 'a', QC_DCO_HEAD: '' }), {
    base: 'a',
    head: 'HEAD',
    repo: undefined,
  });
  assert.equal(parseOptions(['--base=x', '--head', 'y'], {}).head, 'y');
});

// ---- against throwaway repositories --------------------------------------------

function makeRepository() {
  const cwd = mkdtempSync(path.join(tmpdir(), 'qc-dco-'));
  const emptyConfig = `${cwd}.gitconfig`;
  writeFileSync(emptyConfig, '');
  const run = (args, env = {}) =>
    execFileSync('git', args, {
      cwd,
      encoding: 'utf8',
      env: {
        ...process.env,
        GIT_CONFIG_GLOBAL: emptyConfig,
        GIT_CONFIG_NOSYSTEM: '1',
        GIT_AUTHOR_NAME: 'Ada Lovelace',
        GIT_AUTHOR_EMAIL: 'ada@example.com',
        GIT_COMMITTER_NAME: 'Ada Lovelace',
        GIT_COMMITTER_EMAIL: 'ada@example.com',
        ...env,
      },
    });
  run(['init', '--quiet', '--initial-branch=main']);
  run(['config', 'commit.gpgsign', 'false']);
  run(['config', 'core.autocrlf', 'false']);
  const commit = (file, message, env) => {
    mkdirSync(path.dirname(path.join(cwd, file)), { recursive: true });
    writeFileSync(path.join(cwd, file), `${message}\n`);
    run(['add', '--all']);
    run(['commit', '--quiet', '-m', message], env);
    return run(['rev-parse', 'HEAD']).trim();
  };
  return { cwd, emptyConfig, run, commit };
}

function withRepository(body) {
  const repository = makeRepository();
  try {
    body(repository);
  } finally {
    rmSync(repository.cwd, { recursive: true, force: true });
    rmSync(repository.emptyConfig, { force: true });
  }
}

const signoff = 'Signed-off-by: Ada Lovelace <ada@example.com>';

test('only base..head is examined: unsigned history before the base needs no sign-off', () => {
  withRepository(({ cwd, commit }) => {
    commit('a.txt', 'publisher commit without sign-off');
    const base = commit('b.txt', 'another unsigned one');
    const signed = commit('c.txt', `contribution\n\n${signoff}`);
    const result = auditDco({ cwd, base });
    assert.deepEqual(result.commits, [signed]);
    assert.deepEqual(result.errors, []);
  });
});

test('an unsigned commit in the range is reported with its sha', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    commit('b.txt', `signed\n\n${signoff}`);
    const unsigned = commit('c.txt', 'forgot -s');
    assert.deepEqual(auditDco({ cwd, base }).errors, [
      `${unsigned}: no Signed-off-by trailer (commit with \`git commit -s\`)`,
    ]);
  });
});

const spoofedBot = {
  GIT_AUTHOR_NAME: 'dependabot[bot]',
  GIT_AUTHOR_EMAIL: 'x[bot]@users.noreply.github.com',
  GIT_COMMITTER_NAME: 'dependabot[bot]',
  GIT_COMMITTER_EMAIL: 'x[bot]@users.noreply.github.com',
};

test('a commit with a spoofed bot address and no sign-off fails; with a matching sign-off it passes', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const spoofed = commit('b.txt', 'Bump left-pad', spoofedBot);
    assert.deepEqual(auditDco({ cwd, base }).errors, [
      `${spoofed}: no Signed-off-by trailer (commit with \`git commit -s\`)`,
    ]);
    const signed = commit(
      'c.txt',
      'Bump right-pad\n\nSigned-off-by: dependabot[bot] <x[bot]@users.noreply.github.com>',
      spoofedBot,
    );
    const result = auditDco({ cwd, base: spoofed });
    assert.deepEqual(result.commits, [signed]);
    assert.deepEqual(result.errors, []);
  });
});

test('an empty range passes', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    assert.deepEqual(auditDco({ cwd, base }), { commits: [], errors: [] });
  });
});

// Base and topic both rewrite a.txt (each side commit signed); a merge of the
// base into the topic then has to resolve the conflict by hand.
function conflictedMerge({ cwd, run, commit }) {
  commit('a.txt', 'base');
  run(['checkout', '--quiet', '-b', 'topic']);
  commit('a.txt', `topic edit\n\n${signoff}`);
  run(['checkout', '--quiet', 'main']);
  commit('a.txt', `main edit\n\n${signoff}`);
  run(['checkout', '--quiet', 'topic']);
  assert.throws(() => run(['merge', '--no-commit', '--no-ff', 'main'], { GIT_MERGE_AUTOEDIT: 'no' }), /./);
  writeFileSync(path.join(cwd, 'a.txt'), 'hand-resolved\n');
  run(['add', 'a.txt']);
  return run(['rev-parse', 'main']).trim();
}

test('an unsigned merge commit that resolves a conflict fails; signed it passes', () => {
  withRepository((repository) => {
    const { cwd, run } = repository;
    const mainTip = conflictedMerge(repository);
    run(['commit', '--quiet', '-m', 'Merge main into topic']);
    const merge = run(['rev-parse', 'HEAD']).trim();
    assert.deepEqual(auditDco({ cwd, base: mainTip, head: 'topic' }).errors, [
      `${merge}: no Signed-off-by trailer (commit with \`git commit -s\`)`,
    ]);
  });
  withRepository((repository) => {
    const { cwd, run } = repository;
    const mainTip = conflictedMerge(repository);
    run(['commit', '--quiet', '--signoff', '-m', 'Merge main into topic']);
    const result = auditDco({ cwd, base: mainTip, head: 'topic' });
    assert.deepEqual(result.errors, []);
    assert.equal(result.commits.length, 2, 'topic edit and the signed merge');
  });
});

test('a sign-off by someone other than the author fails', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const sha = commit('b.txt', 'x\n\nSigned-off-by: Grace Hopper <grace@example.com>');
    assert.deepEqual(auditDco({ cwd, base }).errors, [
      `${sha}: no Signed-off-by trailer matches the author "Ada Lovelace <ada@example.com>"`,
    ]);
  });
});

test('an unknown base is an error, not a pass', () => {
  withRepository(({ cwd, commit }) => {
    commit('a.txt', 'base');
    assert.throws(() => auditDco({ cwd, base: '0'.repeat(40) }), /is not a commit/);
  });
});
