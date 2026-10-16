import { spawn } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { runOwnedWindowsCommand } from './windows-owned-producer.mjs';

// Operational ceiling, not a correctness assertion or a reason to retry a test.
export const TEST_RUN_TIMEOUT_MS = 20 * 60 * 1000;

export async function runOwnedTests({ argv, cwd = process.cwd(), environment = process.env,
  timeoutMilliseconds = TEST_RUN_TIMEOUT_MS, signalSource = process,
  stdout = process.stdout, stderr = process.stderr, platform = process.platform,
  windowsProducer = runOwnedWindowsCommand } = {}) {
  if (!Array.isArray(argv) || !argv.length || argv.some(arg => typeof arg !== 'string')
      || argv.some(arg => ['--help', '-h', '--watch'].includes(arg))) {
    throw new Error('Supply explicit test paths/Node test options, not help or watch mode.');
  }
  if (!Number.isSafeInteger(timeoutMilliseconds) || timeoutMilliseconds < 1) throw new Error('Invalid test-run deadline.');
  // A new runner is not a worker in its caller's node:test invocation.
  // Inheriting this marker silently skips every nested test file.
  environment = Object.fromEntries(Object.entries(environment).filter(([key]) => key.toUpperCase() !== 'NODE_TEST_CONTEXT'));
  const cancellation = new AbortController();
  let interrupted;
  const interrupt = name => { interrupted ??= name; cancellation.abort(); };
  const sigint = () => interrupt('SIGINT');
  const sigterm = () => interrupt('SIGTERM');
  signalSource.on('SIGINT', sigint);
  signalSource.on('SIGTERM', sigterm);
  try {
    if (platform === 'win32') {
      const result = await windowsProducer({ command: process.execPath,
        arguments: ['--experimental-strip-types', '--test', '--test-concurrency=4', ...argv], cwd, environment,
        nativeTestState: true, timeoutMilliseconds }, {
        signal: cancellation.signal,
        onStdout: text => stdout.write(text), onStderr: text => stderr.write(text),
      });
      if (result.retirementAcknowledged !== true) throw new Error('Test tree cleanup was not acknowledged.');
      return interrupted ? (interrupted === 'SIGINT' ? 130 : 143) : result.code;
    }
    return await new Promise((settle, reject) => {
      // This supervisor survives a killed runner. IPC EOF retires only its own
      // process group and exclusive test-state directory, even after root exit.
      const supervisor = spawn(process.execPath, [fileURLToPath(new URL('./owned-tests-supervisor.mjs', import.meta.url))], {
        detached: true, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
      });
      const forward = (source, sink) => {
        source.on('data', chunk => { try { sink.write(chunk); } catch { cancellation.abort(); } });
      };
      forward(supervisor.stdout, stdout); forward(supervisor.stderr, stderr);
      let receipt;
      const abort = () => { if (supervisor.connected) supervisor.send({ cancel: true }); };
      cancellation.signal.addEventListener('abort', abort, { once: true });
      supervisor.on('error', reject);
      supervisor.on('message', message => { receipt = message; });
      supervisor.once('close', code => {
        cancellation.signal.removeEventListener('abort', abort);
        if (receipt?.retired !== true) reject(new Error('Test tree cleanup was not acknowledged.'));
        else settle(interrupted ? (interrupted === 'SIGINT' ? 130 : 143) : (receipt.code ?? code ?? 1));
      });
      supervisor.send({ argv, cwd, environment, timeoutMilliseconds });
      if (cancellation.signal.aborted) abort();
    });
  } catch (error) {
    if (interrupted && error.retirementAcknowledged === true) return interrupted === 'SIGINT' ? 130 : 143;
    throw error;
  } finally {
    signalSource.removeListener('SIGINT', sigint);
    signalSource.removeListener('SIGTERM', sigterm);
  }
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { process.exitCode = await runOwnedTests({ argv: process.argv.slice(2) }); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
