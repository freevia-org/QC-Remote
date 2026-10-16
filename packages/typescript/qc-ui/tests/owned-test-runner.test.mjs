import assert from 'node:assert/strict';
import test from 'node:test';
import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runOwnedTests } from '../../../../scripts/run-owned-tests.mjs';

test('owned test runner preserves exact argv/outcome and confines state under the existing Windows owner', async () => {
  const signals = new EventEmitter();
  const out = [];
  const argv = ['--test-name-pattern=a b', 'a.test.mjs'];
  const environment = { KEEP: 'value' };
  const code = await runOwnedTests({ argv, environment, signalSource: signals, platform: 'win32',
    stdout: { write: text => out.push(text) }, stderr: { write: text => out.push(text) },
    windowsProducer: async (request, options) => {
      assert.deepEqual(request.arguments, ['--experimental-strip-types', '--test', '--test-concurrency=4', ...argv]);
      assert.equal(request.nativeTestState, true);
      assert.deepEqual(request.environment, environment);
      assert.ok(request.timeoutMilliseconds > 0);
      options.onStdout('out'); options.onStderr('err');
      return { code: 17, retirementAcknowledged: true };
    } });
  assert.equal(code, 17);
  assert.deepEqual(out, ['out', 'err']);
  assert.equal(signals.listenerCount('SIGINT') + signals.listenerCount('SIGTERM'), 0);
});

test('interrupt waits for retirement and restores handlers instead of exiting early', async () => {
  const signals = new EventEmitter();
  let cancelled = false;
  const code = await runOwnedTests({ argv: ['a.test.mjs'], platform: 'win32', signalSource: signals,
    windowsProducer: async (_request, { signal }) => {
      signal.addEventListener('abort', () => { cancelled = true; });
      signals.emit('SIGINT');
      assert.equal(cancelled, true);
      throw Object.assign(new Error('cancelled after cleanup'), { retirementAcknowledged: true });
    } });
  assert.equal(code, 130);
  assert.equal(signals.listenerCount('SIGINT'), 0);
  await assert.rejects(runOwnedTests({ argv: ['--help'] }), /explicit test paths/);
  await assert.rejects(runOwnedTests({ argv: ['a.test.mjs'], platform: 'win32',
    windowsProducer: async () => ({ code: 0 }) }), /cleanup was not acknowledged/);
});

const fixture = fileURLToPath(new URL('../../../../scripts/fixtures/owned-ui-test.mjs', import.meta.url));
const driver = fileURLToPath(new URL('../../../../scripts/fixtures/owned-ui-test-driver.mjs', import.meta.url));
const alive = pid => { try { process.kill(pid, 0); return true; } catch (error) { if (error.code === 'ESRCH') return false; throw error; } };
async function until(check, message, limit = 90_000) {
  const start = Date.now();
  while (!(await check())) {
    if (Date.now() - start > limit) throw new Error(message);
    await new Promise(resolve => setTimeout(resolve, 50));
  }
}

const chrome = process.platform === 'win32' && existsSync('C:/Program Files/Google/Chrome/Application/chrome.exe')
  ? 'C:/Program Files/Google/Chrome/Application/chrome.exe' : undefined;
for (const mode of ['success', 'failure', 'worker-killed', 'owner-killed', 'interrupt', 'deadline', ...(chrome ? ['browser-owner-killed'] : [])]) {
  test(`real owned test run retires descendants and partial profile on ${mode}`, { timeout: 180_000 }, async () => {
    const control = await mkdtemp(join(tmpdir(), 'qc-test-owner-proof-'));
    const report = join(control, 'report.json');
    const sibling = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
    let child;
    let state;
    try {
      const options = { argv: [fixture], environment: { ...process.env, ...(mode === 'browser-owner-killed' ? { QC_FIXTURE_BROWSER: chrome } : {}), QC_FIXTURE_REPORT: report,
        QC_FIXTURE_MODE: ['success', 'failure'].includes(mode) ? mode : 'hang' }, timeoutMilliseconds: mode === 'deadline' ? 15_000 : 90_000 };
      child = spawn(process.execPath, [driver], { stdio: ['ignore', 'pipe', 'pipe', 'ipc'] });
      child.send(options);
      let output = '';
      child.stdout.on('data', data => { output = (output + data).slice(-16_384); });
      child.stderr.on('data', data => { output = (output + data).slice(-16_384); });
      const closed = new Promise(resolve => child.once('close', (code, signal) => resolve({ code, signal })));
      await until(() => existsSync(report) || child.exitCode !== null, 'fixture never became ready');
      assert.ok(existsSync(report), `no fixture dispatched: ${output}`);
      state = JSON.parse(await readFile(report, 'utf8'));
      assert.notEqual(state.root, process.env.QC_REMOTE_TEST_STATE_ROOT, 'an inherited path is not the new run owner');
      if (mode === 'owner-killed' || mode === 'browser-owner-killed') child.kill('SIGKILL');
      if (mode === 'worker-killed') process.kill(state.worker, 'SIGKILL');
      if (mode === 'interrupt') child.send('SIGTERM');
      await until(() => child.exitCode !== null || child.signalCode !== null, `test runner did not settle: ${output}`);
      const outcome = await closed;
      if (mode === 'success') assert.equal(outcome.code, 0, output);
      else assert.ok(outcome.code !== 0 || outcome.signal, `failed/cancelled test falsely succeeded: ${output}`);
      await until(() => !alive(state.child) && !alive(state.worker) && !existsSync(state.root), `owned processes or bytes survived: ${JSON.stringify(state)} ${output}`);
      assert.equal(alive(sibling.pid), true, 'unrelated sibling must survive every failure path');
    } finally {
      if (child && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      sibling.kill();
      await until(() => !alive(sibling.pid), 'unrelated control process cleanup incomplete');
      if (state) await until(() => !alive(state.child) && !existsSync(state.root), 'fixture cleanup incomplete');
      await rm(control, { recursive: true, force: true });
    }
  });
}
