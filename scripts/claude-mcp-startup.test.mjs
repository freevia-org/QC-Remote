import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const bridge = fileURLToPath(new URL('../apps/windows/src-tauri/local-clients/bridge.cjs', import.meta.url));
const requests = [
  { jsonrpc: '2.0', id: 0, method: 'initialize', params: {
    protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'startup-test', version: '1' },
  } },
  { jsonrpc: '2.0', id: 1, method: 'ping' },
];

for (const mode of ['direct', 'dynamic-import', 'claude-stdin-proxy']) {
  test(`Claude bridge answers initialize and ping through ${mode}`, () => {
    const directory = mkdtempSync(join(tmpdir(), 'qc-claude-startup-'));
    try {
      // Claude's nodeHost.js imports the server from its own main module after
      // replacing argv and proxying stdin methods onto a MessagePort Readable.
      const host = `
        const { pathToFileURL } = require('node:url');
        const target = process.argv[1];
        if (${JSON.stringify(mode)} === 'claude-stdin-proxy') {
          process.parentPort = {};
          const input = require('node:fs').readFileSync(0);
          const proxy = new (require('node:stream').Readable)({ read() {} });
          for (const key of ['read','push','unshift','pause','resume','pipe','unpipe','on','once','removeListener','removeAllListeners','setEncoding','destroy','isPaused']) {
            process.stdin[key] = proxy[key].bind(proxy);
          }
          for (const key of ['readableHighWaterMark','readableLength','destroyed']) {
            Object.defineProperty(process.stdin, key, { get: () => proxy[key], configurable: true });
          }
          proxy.push(input); proxy.push(null);
        }
        process.argv = [process.execPath, target];
        import(pathToFileURL(target).href).catch(error => { console.error(error); process.exitCode = 1; });
      `;
      const result = spawnSync(process.execPath, mode === 'direct' ? [bridge] : ['-e', host, bridge], {
        input: requests.map(request => JSON.stringify(request)).join('\n') + '\n',
        encoding: 'utf8', windowsHide: true, timeout: 5000,
        env: { ...process.env, QC_REMOTE_LOCAL_MCP_STATE: join(directory, 'missing.json') },
      });
      assert.equal(result.error, undefined, result.stderr);
      assert.equal(result.status, 0, result.stderr);
      const replies = result.stdout.trim().split('\n').filter(Boolean).map(line => JSON.parse(line));
      assert.equal(replies.length, 2, `missing handshake replies: ${result.stdout} ${result.stderr}`);
      assert.equal(replies.find(reply => reply.id === 0)?.result.protocolVersion, '2025-06-18');
      assert.deepEqual(replies.find(reply => reply.id === 1)?.result, {});
    } finally {
      rmSync(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    }
  });
}

test('ordinary bridge library imports do not start the stdio server', () => {
  const result = spawnSync(process.execPath, ['-e', `
    const bridge = require(process.argv[2]);
    console.log(typeof bridge.main);
  `, 'different-host-entry.cjs', bridge], { encoding: 'utf8', windowsHide: true, timeout: 5000 });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), 'function');
});
