import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// POSIX only. Windows uses the existing original-handle/Job-object producer.
if (process.platform === 'win32' || !process.send) throw new Error('An IPC-owning POSIX runner is required.');
let child;
let root;
let retiring;
let cancelled = false;
let timer;
const killGroup = signal => {
  if (!child?.pid) return;
  try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; }
};
const retire = code => {
  if (retiring) return retiring;
  retiring = (async () => {
    clearTimeout(timer);
    try {
      // Descendants can retain pipes after their original root exits. Group
      // lifetime, not the root ChildProcess.exitCode, decides cleanup here.
      killGroup('SIGKILL');
      if (child?.pid && child.exitCode === null && child.signalCode === null) {
        await new Promise(resolve => child.once('exit', resolve));
      }
      if (root) await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      if (process.connected) { process.send({ retired: true, code }); process.disconnect(); }
      process.exitCode = 0;
    } catch (error) {
      console.error(`Test cleanup failed; state retained at ${root}: ${error.message}`);
      process.exitCode = 1;
      if (process.connected) { process.send({ retired: false }); process.disconnect(); }
    }
  })();
  return retiring;
};
const cancel = () => { cancelled = true; if (child) void retire(1); };
process.on('disconnect', cancel);
process.on('SIGINT', cancel);
process.on('SIGTERM', cancel);
process.stdout.on('error', cancel);
process.stderr.on('error', cancel);
let admitted = false;
process.on('message', async request => {
  if (request.cancel) { cancel(); return; }
  if (admitted) return;
  admitted = true;
  try {
    root = await mkdtemp(join(request.environment.QC_REMOTE_TEST_STATE_ROOT ?? tmpdir(), 'qc-owned-tests-'));
    if (cancelled) { await retire(1); return; }
    const environment = Object.fromEntries(Object.entries(request.environment).filter(([key]) => key.toUpperCase() !== 'QC_REMOTE_TEST_STATE_ROOT'));
    child = spawn(process.execPath, ['--experimental-strip-types', '--test', '--test-concurrency=4', ...request.argv], {
      cwd: request.cwd, env: { ...environment, QC_REMOTE_TEST_STATE_ROOT: root },
      detached: true, stdio: ['ignore', 'pipe', 'pipe'],
    });
    child.stdout.on('data', data => process.stdout.write(data));
    child.stderr.on('data', data => process.stderr.write(data));
    child.once('error', () => void retire(1));
    child.once('exit', code => void retire(code ?? 1));
    timer = setTimeout(() => { console.error('Test-run deadline expired.'); void retire(1); }, request.timeoutMilliseconds);
  } catch (error) { console.error(error.message); await retire(1); }
});
