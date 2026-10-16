import assert from 'node:assert/strict';
import test from 'node:test';
import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm, chmod } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runOwnedGradle } from './run-owned-gradle.mjs';
import { buildGradleEnvironment, buildGradleOptions } from './gradle-build-environment.mjs';

test('Gradle build policy overrides daemon requests without mutating caller options', () => {
  const original = { Gradle_Opts: '-Xmx1g -Dorg.gradle.daemon=true -Dorg.gradle.project.kotlin.compiler.execution.strategy=daemon', KEEP: 'preserved' };
  const env = buildGradleEnvironment(original);
  assert.equal(env.GRADLE_OPTS, '-Xmx1g -Dorg.gradle.daemon=false -Dorg.gradle.project.kotlin.compiler.execution.strategy=in-process');
  assert.equal(env.Gradle_Opts, undefined);
  assert.equal(env.KEEP, 'preserved');
  assert.match(original.Gradle_Opts, /daemon=true/);
  assert.equal(buildGradleOptions(env.GRADLE_OPTS), env.GRADLE_OPTS);
});

test('Owned Gradle forwards exact arguments and requires affirmative retirement', async () => {
  const signals = new EventEmitter();
  const output = [];
  const code = await runOwnedGradle({wrapper: '/wrapper', argv: ['-Pname=two words'], signals, platform: 'win32',
    stdout: {write: data => output.push(data)}, stderr: {write: data => output.push(data)},
    windowsProducer: async (request, callbacks) => {
      assert.equal(request.command, process.execPath);
      assert.deepEqual(request.arguments.slice(-3), ['-Pname=two words', '--no-daemon', '-Pkotlin.compiler.execution.strategy=in-process']);
      assert.match(request.environment.GRADLE_OPTS, /org.gradle.daemon=false/);
      callbacks.onStdout('out'); callbacks.onStderr('err');
      return {code: 19, retirementAcknowledged: true};
    }});
  assert.equal(code, 19); assert.deepEqual(output, ['out', 'err']);
  assert.equal(signals.listenerCount('SIGINT') + signals.listenerCount('SIGTERM'), 0);
  await assert.rejects(runOwnedGradle({wrapper: '/wrapper', platform: 'win32', windowsProducer: async () => ({code: 0})}), /retirement was not acknowledged/);
  await assert.rejects(runOwnedGradle({wrapper: '/wrapper', argv: ['--stop']}), /Shared daemon management/);
});

test('Build cancellation waits for owned retirement and restores its signal handlers', async () => {
  const signals = new EventEmitter();
  const code = await runOwnedGradle({wrapper: '/wrapper', signals, platform: 'win32', windowsProducer: async (_request, {signal}) => {
    signals.emit('SIGTERM'); assert.equal(signal.aborted, true);
    throw Object.assign(new Error('cancelled after retirement'), {retirementAcknowledged: true});
  }});
  assert.equal(code, 143); assert.equal(signals.listenerCount('SIGTERM'), 0);
});

const implementation = new URL('./run-owned-gradle.mjs', import.meta.url).href;
const alive = pid => {try {process.kill(pid, 0); return true;} catch (error) {if (error.code === 'ESRCH') return false; throw error;}};
async function until(check, description, timeout = 90_000) {
  const end = Date.now() + timeout;
  while (!await check()) {if (Date.now() > end) throw new Error(description); await new Promise(resolve => setTimeout(resolve, 50));}
}

for (const mode of ['success', 'failure', 'interruption', 'killed-launcher', 'deadline']) {
  test(`Real owned build retires a compiler grandchild on ${mode} without touching a sibling`, {timeout: 150_000}, async () => {
    const directory = await mkdtemp(join(process.env.QC_REMOTE_TEST_STATE_ROOT ?? tmpdir(), 'qc-gradle-proof-'));
    const report = join(directory, 'report.json');
    const fixture = join(directory, 'fake compiler.mjs');
    const wrapper = join(directory, process.platform === 'win32' ? 'gradlew.bat' : 'gradlew');
    const sibling = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {stdio:'ignore'});
    let driver, state;
    try {
      await writeFile(fixture, `import {spawn} from 'node:child_process'; import {writeFileSync} from 'node:fs';
const child=spawn(process.execPath,['-e','setInterval(() => {}, 1000)'],{stdio:'ignore'}); child.unref();
writeFileSync(process.env.QC_BUILD_REPORT,JSON.stringify({worker:process.pid,child:child.pid,argv:process.argv.slice(2),opts:process.env.GRADLE_OPTS}));
if(process.env.QC_BUILD_MODE==='success') process.exitCode=0;
else if(process.env.QC_BUILD_MODE==='failure') process.exitCode=23;
else setInterval(() => {},1000);
`);
      await writeFile(wrapper, process.platform === 'win32' ? `@echo off\r\n"${process.execPath}" "${fixture}" %*\r\n` : `#!/bin/sh\nexec "${process.execPath}" "${fixture}" "$@"\n`);
      if (process.platform !== 'win32') await chmod(wrapper, 0o700);
      driver = spawn(process.execPath, ['--input-type=module', '-e', `
import {runOwnedGradle} from ${JSON.stringify(implementation)};
const options=await new Promise(resolve=>process.once('message',resolve));
process.on('message', message=>{if(message==='interrupt')process.emit('SIGINT')});
try {process.exitCode=await runOwnedGradle(options)}catch(error){console.error(error.message);process.exitCode=1}
finally {if(process.connected)process.disconnect()}
`], {stdio:['ignore','pipe','pipe','ipc']});
      let output = '';
      for (const pipe of [driver.stdout, driver.stderr]) pipe.on('data', data => {output=(output+data).slice(-16_384);});
      driver.send({wrapper,cwd:directory,argv:['-Pfixture=two words'],environment:{...process.env,QC_BUILD_REPORT:report,QC_BUILD_MODE:mode},timeoutMilliseconds:mode==='deadline'?8000:90_000});
      await until(()=>existsSync(report)||driver.exitCode!==null,`Fixture startup did not settle: ${output}`);
      assert.ok(existsSync(report), `No build dispatched: ${output}`);
      state=JSON.parse(await readFile(report,'utf8'));
      assert.deepEqual(state.argv.slice(-2), ['--no-daemon','-Pkotlin.compiler.execution.strategy=in-process']);
      assert.ok(state.argv.includes('-Pfixture=two words'), 'batch quoting must preserve spaced arguments');
      assert.match(state.opts,/org.gradle.daemon=false/);
      if(mode==='killed-launcher') driver.kill('SIGKILL');
      if(mode==='interruption') driver.send('interrupt');
      await until(()=>driver.exitCode!==null||driver.signalCode!==null,`Build did not settle: ${output}`);
      if(mode==='success') assert.equal(driver.exitCode,0,output);
      else if(mode==='failure') assert.equal(driver.exitCode,23,output);
      else if(mode==='interruption') assert.equal(driver.exitCode,130,output);
      else assert.ok(driver.exitCode!==0||driver.signalCode,`Failed build reported success: ${output}`);
      await until(()=>!alive(state.worker)&&!alive(state.child),`Compiler tree survived: ${JSON.stringify(state)} ${output}`);
      assert.equal(alive(sibling.pid),true,'Unrelated active session must survive');
    } finally {
      if(driver&&driver.exitCode===null&&driver.signalCode===null) driver.kill('SIGKILL');
      if(state) await until(()=>!alive(state.worker)&&!alive(state.child),'Build cleanup incomplete');
      sibling.kill(); await until(()=>!alive(sibling.pid),'Control process cleanup incomplete');
      // Hard owner death is asynchronous: the surviving native supervisor may
      // still be closing the admitted worker's CWD after its compiler exits.
      // Keep failure visible if bounded retirement/deletion cannot complete.
      await rm(directory,{recursive:true,force:true,maxRetries:8,retryDelay:100});
    }
  });
}
