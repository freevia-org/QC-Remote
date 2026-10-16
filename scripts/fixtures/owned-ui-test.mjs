import test from 'node:test';
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

test('owned test fixture', async () => {
  const root = process.env.QC_REMOTE_TEST_STATE_ROOT;
  if (!root) throw new Error('Fixture has no original-owned state root');
  const profile = join(root, 'fixture-profile');
  mkdirSync(profile);
  writeFileSync(join(profile, 'partial'), 'test-only bytes');
  const child = process.env.QC_FIXTURE_BROWSER
    ? spawn(process.env.QC_FIXTURE_BROWSER, ['--headless=new', '--disable-gpu', '--no-sandbox', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'] })
    : spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
  await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
  let diagnostic = '';
  child.stderr?.on('data', chunk => { diagnostic = (diagnostic + chunk).slice(-4096); });
  child.unref();
  if (process.env.QC_FIXTURE_BROWSER) {
    const deadline = Date.now() + 30_000;
    while (!existsSync(join(profile, 'DevToolsActivePort'))) {
      if (child.exitCode !== null || Date.now() > deadline) throw new Error(`Fixture Chrome never became ready (exit ${child.exitCode}, profile length ${profile.length}): ${diagnostic}`);
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  }
  writeFileSync(process.env.QC_FIXTURE_REPORT, JSON.stringify({ worker: process.pid, child: child.pid, root, profile }));
  if (process.env.QC_FIXTURE_MODE === 'failure') throw new Error('deliberate fixture assertion failure');
  if (process.env.QC_FIXTURE_MODE === 'success') return;
  await new Promise(() => { setInterval(() => {}, 1000); });
});
