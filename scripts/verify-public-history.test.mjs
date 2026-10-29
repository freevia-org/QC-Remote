import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  auditHistory,
  identityErrors,
  isPublishedIdentityException,
  parseOptions,
  publishedIdentityExceptions,
} from './verify-public-history.mjs';

const noreply = '1+someone@users.noreply.github.com';

test('a noreply author and committer pass', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: noreply, committerName: 'someone', committerEmail: noreply }),
    [],
  );
});

test('a GitHub-made merge (web-flow committer) passes', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: noreply, committerName: 'GitHub', committerEmail: 'noreply@github.com' }),
    [],
  );
});

test('the web-flow address is not accepted as an author', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: 'noreply@github.com', committerName: 'GitHub', committerEmail: 'noreply@github.com' }),
    ['author email is not a noreply identity'],
  );
});

test('the web-flow address under any other committer name is rejected', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: noreply, committerName: 'someone', committerEmail: 'noreply@github.com' }),
    ['committer email is not a noreply identity'],
  );
});

test('a personal address is rejected in either role', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: 'person@example.com', committerName: 'GitHub', committerEmail: 'person@example.com' }),
    ['author email is not a noreply identity', 'committer email is not a noreply identity'],
  );
});

test('the organisation publisher address is accepted as author and committer', () => {
  assert.deepEqual(
    identityErrors({ authorEmail: 'info@freevia.org', committerName: 'Freevia', committerEmail: 'info@freevia.org' }),
    [],
  );
  assert.deepEqual(
    identityErrors({ authorEmail: ' Info@Freevia.ORG ', committerName: 'Freevia', committerEmail: 'INFO@FREEVIA.ORG' }),
    [],
  );
});

test('only the exact organisation address is allowed, never look-alikes or personal addresses', () => {
  for (const email of [
    'info@freevia.org.evil.com',
    'Info@FREEVIA.ORG.example',
    'xinfo@freevia.org',
    'info@mail.freevia.org',
    'security@freevia.org',
    'someone@gmail.com',
  ]) {
    assert.deepEqual(
      identityErrors({ authorEmail: email, committerName: 'x', committerEmail: email }),
      ['author email is not a noreply identity', 'committer email is not a noreply identity'],
      email,
    );
  }
});

// ---- argument parsing -------------------------------------------------------

test('no arguments means whole-history, strict', () => {
  assert.deepEqual(parseOptions([], {}), {
    base: undefined,
    head: undefined,
    identity: 'strict',
    repo: undefined,
  });
});

test('--base, --head and --identity are read from flags, with = or a separate value', () => {
  assert.deepEqual(parseOptions(['--base', 'aaa', '--head=bbb', '--identity', 'advisory'], {}), {
    base: 'aaa',
    head: 'bbb',
    identity: 'advisory',
    repo: undefined,
  });
});

test('environment variables fill in what flags leave out; an empty variable is unset', () => {
  assert.equal(parseOptions([], { QC_HISTORY_BASE: 'abc' }).base, 'abc');
  assert.equal(parseOptions([], { QC_HISTORY_BASE: '' }).base, undefined);
  assert.equal(parseOptions(['--base', 'flag'], { QC_HISTORY_BASE: 'env' }).base, 'flag');
});

test('an empty flag value is an error, never a silent fallback to another mode', () => {
  assert.throws(() => parseOptions(['--base', ''], {}), /--base must not be empty/);
  assert.throws(() => parseOptions(['--base='], {}), /--base must not be empty/);
  assert.throws(() => parseOptions(['--base'], {}), /--base needs a value/);
});

test('unknown flags, a stray --head and a bad --identity are rejected', () => {
  assert.throws(() => parseOptions(['--bse', 'x'], {}), /unknown argument/);
  assert.throws(() => parseOptions(['--head', 'x'], {}), /--head only applies together with --base/);
  assert.throws(() => parseOptions(['--base', 'x', '--identity', 'lax'], {}), /--identity must be/);
});

test('whole-history mode cannot be relaxed', () => {
  assert.throws(() => parseOptions(['--identity', 'advisory'], {}), /only applies to range mode/);
});

// ---- audits against throwaway repositories ----------------------------------

const noreplyAuthor = '1+someone@users.noreply.github.com';
const realAuthor = 'contributor@example.com';

function makeRepository() {
  const cwd = mkdtempSync(path.join(tmpdir(), 'qc-history-'));
  // An empty global config beside (not inside) the repository, so the developer's
  // own git settings cannot leak into the fixture.
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
        GIT_AUTHOR_NAME: 'someone',
        GIT_AUTHOR_EMAIL: noreplyAuthor,
        GIT_COMMITTER_NAME: 'someone',
        GIT_COMMITTER_EMAIL: noreplyAuthor,
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

test('range mode checks only the commits the range adds', () => {
  withRepository(({ cwd, commit }) => {
    // History before the base is dirty on purpose: a range run must not see it.
    commit('a.txt', 'old history', { GIT_AUTHOR_EMAIL: realAuthor });
    const base = commit('b.txt', 'base');
    commit('c.txt', 'pr work');
    const result = auditHistory({ cwd, base });
    assert.equal(result.commits.length, 1);
    assert.deepEqual(result.errors, []);
    assert.equal(auditHistory({ cwd }).errors.length > 0, true, 'whole-history mode still sees the old commit');
  });
});

test('whole-history mode fails on a Co-authored-by trailer', () => {
  withRepository(({ cwd, commit }) => {
    const sha = commit('a.txt', 'work\n\nCo-authored-by: Someone <someone@example.com>');
    const { errors } = auditHistory({ cwd });
    assert.deepEqual(errors, [`${sha}: commit message contains a Co-authored-by trailer`]);
  });
});

test('published identity exceptions are exact and cannot weaken later history', () => {
  const publishedCommits = [
    '2badc68ea4d30f53fc98ae9e000c70df109d0970',
    'db44c774a284c0f189e23cbdfd0ff9265b7ae400',
    'b5cc7ff4c6192c6211cce1a455350c28bea88b4d',
    '462997bbd6d06f67aeff811d44ef50ec98e71a58',
  ];
  const trailerFinding = 'commit message contains a Co-authored-by trailer';
  assert.equal(publishedIdentityExceptions.size, publishedCommits.length);
  for (const publishedCommit of publishedCommits) {
    assert.equal(isPublishedIdentityException(publishedCommit, trailerFinding), true);
    assert.equal(
      isPublishedIdentityException(publishedCommit, 'author email is not a noreply identity'),
      false,
    );
  }
  assert.equal(isPublishedIdentityException('0'.repeat(40), trailerFinding), false);
});

test('range mode, strict identity, still rejects a real author address and a trailer', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const sha = commit('b.txt', 'pr\n\nCo-authored-by: A <a@example.com>', { GIT_AUTHOR_EMAIL: realAuthor });
    const { errors, warnings } = auditHistory({ cwd, base });
    assert.deepEqual(warnings, []);
    assert.deepEqual(errors, [
      `${sha}: author email is not a noreply identity`,
      `${sha}: commit message contains a Co-authored-by trailer`,
    ]);
  });
});

test('range mode, advisory identity, reports a sign-off author and a trailer as warnings only', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const sha = commit(
      'b.txt',
      'pr\n\nSigned-off-by: A Contributor <contributor@example.com>\nCo-authored-by: B <b@example.com>',
      { GIT_AUTHOR_EMAIL: realAuthor, GIT_COMMITTER_EMAIL: realAuthor },
    );
    const { errors, warnings } = auditHistory({ cwd, base, identity: 'advisory' });
    assert.deepEqual(errors, []);
    assert.deepEqual(warnings, [
      `${sha}: author email is not a noreply identity`,
      `${sha}: committer email is not a noreply identity`,
      `${sha}: commit message contains a Co-authored-by trailer`,
    ]);
  });
});

test('range mode, advisory identity, still fails on a private path', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const sha = commit('references/private/notes.txt', 'pr', { GIT_AUTHOR_EMAIL: realAuthor });
    const { errors } = auditHistory({ cwd, base, identity: 'advisory' });
    assert.deepEqual(errors, [`${sha}: forbidden public path references/private/notes.txt`]);
  });
});

test('a private path added and removed within the range is still caught on the commit that held it', () => {
  withRepository(({ cwd, run, commit }) => {
    const base = commit('a.txt', 'base');
    const leaky = commit('.claude/settings.json', 'leaky');
    run(['rm', '--quiet', '-r', '.claude']);
    run(['commit', '--quiet', '-m', 'remove it again']);
    const { errors } = auditHistory({ cwd, base });
    assert.deepEqual(errors, [`${leaky}: forbidden public path .claude/settings.json`]);
  });
});

test('an empty range is clean, and an unknown base is an error rather than a pass', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    assert.deepEqual(auditHistory({ cwd, base }).commits, []);
    assert.throws(() => auditHistory({ cwd, base: '0'.repeat(40) }), /is not a commit in this repository/);
    assert.throws(() => auditHistory({ cwd, base, head: 'no-such-ref' }), /is not a commit in this repository/);
  });
});

test('--head limits the range, so later commits on the branch are not examined', () => {
  withRepository(({ cwd, commit }) => {
    const base = commit('a.txt', 'base');
    const head = commit('b.txt', 'ok');
    commit('.env.local', 'too late');
    assert.deepEqual(auditHistory({ cwd, base, head }).errors, []);
    assert.equal(auditHistory({ cwd, base }).errors.length, 1);
  });
});
