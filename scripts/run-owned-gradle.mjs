import { spawn } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { resolve } from 'node:path';
import { runOwnedWindowsCommand } from './windows-owned-producer.mjs';
import { buildGradleEnvironment } from './gradle-build-environment.mjs';

export async function runOwnedGradle({ wrapper, argv = [], cwd = process.cwd(), environment = process.env,
  timeoutMilliseconds = 60 * 60 * 1000, signals = process,
  stdout = process.stdout, stderr = process.stderr, platform = process.platform,
  windowsProducer = runOwnedWindowsCommand } = {}) {
  if (typeof wrapper !== 'string' || !Array.isArray(argv) || argv.some(arg => typeof arg !== 'string')) throw new Error('Invalid Gradle command.');
  if (argv.some(arg => ['--stop', '--foreground'].includes(arg))) throw new Error('Shared daemon management is not allowed inside an owned build.');
  if (!Number.isSafeInteger(timeoutMilliseconds) || timeoutMilliseconds < 1) throw new Error('Invalid build deadline.');
  const command = process.execPath;
  const args = [fileURLToPath(new URL('./gradle-build-worker.mjs', import.meta.url)), resolve(wrapper), ...argv,
    '--no-daemon', '-Pkotlin.compiler.execution.strategy=in-process'];
  const env = buildGradleEnvironment(environment);
  const cancellation = new AbortController();
  let interrupted;
  const interrupt = name => { interrupted ??= name; cancellation.abort(); };
  const sigint = () => interrupt('SIGINT'), sigterm = () => interrupt('SIGTERM');
  signals.on('SIGINT', sigint); signals.on('SIGTERM', sigterm);
  try {
    if (platform === 'win32') {
      const result = await windowsProducer({ command, arguments: args, cwd, environment: env, timeoutMilliseconds }, {
        signal: cancellation.signal, onStdout: text => stdout.write(text), onStderr: text => stderr.write(text),
      });
      if (result.retirementAcknowledged !== true) throw new Error('Gradle process retirement was not acknowledged.');
      return interrupted ? (interrupted === 'SIGINT' ? 130 : 143) : result.code;
    }
    return await new Promise((settle, reject) => {
      const supervisor = spawn(process.execPath, [fileURLToPath(new URL('./owned-tests-supervisor.mjs', import.meta.url))], {
        detached: true, stdio: ['ignore', 'pipe', 'pipe', 'ipc'],
      });
      let receipt;
      const abort = () => { if (supervisor.connected) supervisor.send({ cancel: true }); };
      cancellation.signal.addEventListener('abort', abort, { once: true });
      supervisor.stdout.on('data', chunk => stdout.write(chunk));
      supervisor.stderr.on('data', chunk => stderr.write(chunk));
      supervisor.on('error', reject);
      supervisor.on('message', message => { receipt = message; });
      supervisor.once('close', () => {
        cancellation.signal.removeEventListener('abort', abort);
        if (receipt?.retired !== true) reject(new Error('Gradle process retirement was not acknowledged.'));
        else settle(interrupted ? (interrupted === 'SIGINT' ? 130 : 143) : receipt.code);
      });
      supervisor.send({ command, arguments: args, cwd, environment: env, timeoutMilliseconds });
      if (cancellation.signal.aborted) abort();
    });
  } catch (error) {
    if (interrupted && error.retirementAcknowledged === true) return interrupted === 'SIGINT' ? 130 : 143;
    throw error;
  } finally {
    signals.removeListener('SIGINT', sigint); signals.removeListener('SIGTERM', sigterm);
  }
}
if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  try { process.exitCode = await runOwnedGradle({ wrapper: process.argv[2], argv: process.argv.slice(3) }); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
