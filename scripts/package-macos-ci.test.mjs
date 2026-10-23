import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./package-macos-ci.sh', import.meta.url));

function packageImage(mode) {
  const root = mkdtempSync(join(tmpdir(), 'qc-dmg-test-'));
  try {
    const bin = join(root, 'bin');
    mkdirSync(bin);
    const app = join(root, 'QC Remote.app');
    mkdirSync(join(app, 'Contents/Resources/QC Device Broker.app'), { recursive: true });
    writeFileSync(join(app, 'Contents/Resources/QC Device Broker.app/broker'), 'broker');
    const mock = join(root, 'hdiutil.cjs');
    writeFileSync(mock, `
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
if (args[0] === 'verify') process.exit(process.env.QC_TEST_MODE === 'verify-failure' ? 1 : 0);
const counter = process.env.QC_TEST_COUNTER;
const count = fs.existsSync(counter) ? Number(fs.readFileSync(counter, 'utf8')) + 1 : 1;
fs.writeFileSync(counter, String(count));
const source = args[args.indexOf('-srcfolder') + 1];
if (!fs.existsSync(path.join(source, 'QC Remote.app')) ||
    !fs.existsSync(path.join(source, 'QC Device Broker.app/broker')) ||
    fs.readlinkSync(path.join(source, 'Applications')) !== '/Applications') process.exit(9);
const mode = process.env.QC_TEST_MODE;
if (mode === 'busy' || (mode === 'transient' && count < 3)) {
  console.error('hdiutil: create failed - Resource busy'); process.exit(1);
}
if (mode === 'permanent') { console.error('Permission denied'); process.exit(1); }
fs.writeFileSync(args.at(-1), 'verified image');
`);
    writeFileSync(join(bin, 'hdiutil'), '#!/bin/sh\nexec "$QC_TEST_NODE" "$QC_TEST_HDIUTIL" "$@"\n', { mode: 0o755 });
    writeFileSync(join(bin, 'ditto'), '#!/bin/sh\nexec cp -R "$1" "$2"\n', { mode: 0o755 });
    writeFileSync(join(bin, 'sleep'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    const counter = join(root, 'counter');
    const output = join(root, 'output directory/installer.dmg');
    const result = spawnSync('sh', [script, app, output], {
      encoding: 'utf8', timeout: 10000,
      env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, QC_TEST_NODE: process.execPath,
        QC_TEST_HDIUTIL: mock, QC_TEST_COUNTER: counter, QC_TEST_MODE: mode },
    });
    assert.ifError(result.error);
    return { status: result.status, attempts: Number(readFileSync(counter, 'utf8')),
      published: existsSync(output), stderr: result.stderr };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test('retries transient busy images and publishes both apps after verification', () => {
  const result = packageImage('transient');
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.attempts, 3);
  assert.equal(result.published, true);
});

test('bounds busy-image retries and leaves no failed output', () => {
  const result = packageImage('busy');
  assert.notEqual(result.status, 0);
  assert.equal(result.attempts, 4);
  assert.equal(result.published, false);
});

test('fails immediately for permanent image errors', () => {
  const result = packageImage('permanent');
  assert.notEqual(result.status, 0);
  assert.equal(result.attempts, 1);
  assert.equal(result.published, false);
});

test('never publishes an image that fails verification', () => {
  const result = packageImage('verify-failure');
  assert.notEqual(result.status, 0);
  assert.equal(result.attempts, 1);
  assert.equal(result.published, false);
});
