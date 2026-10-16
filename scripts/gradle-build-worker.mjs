import { spawnSync } from 'node:child_process';
import { isAbsolute } from 'node:path';
const [wrapper, ...args] = process.argv.slice(2);
if (!wrapper || !isAbsolute(wrapper)) throw new Error('An absolute Gradle wrapper path is required.');
let command = wrapper, parameters = args;
if (process.platform === 'win32') {
  // cmd expands percent/exclamation even inside quotes. Refuse unsupported
  // shell syntax rather than interpolate it or expose a signing value.
  const quote = value => {
    if (/["%\!\r\n]/.test(value)) throw new Error('Unsupported batch argument syntax.');
    return `"${value}"`;
  };
  command = process.env.ComSpec ?? 'C:/Windows/System32/cmd.exe';
  parameters = ['/d', '/v:off', '/s', '/c', `call ${[wrapper, ...args].map(quote).join(' ')}`];
}
const result = spawnSync(command, parameters, { stdio: 'inherit', windowsHide: true,
  ...(process.platform === 'win32' ? { windowsVerbatimArguments: true } : {}) });
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
