import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { runOwnedWindowsCommand, retainedWindowsProducerCount, reapRetiredWindowsProducers, resolvePowerShell7 } from "./windows-owned-producer.mjs";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, writeFileSync, mkdirSync, rmSync } from "node:fs";
import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";

function successfulProtocolHost(observe) {
  return () => {
    const child = Object.assign(new EventEmitter(), {pid: 10001});
    child.stdin = new PassThrough(); child.stdout = new PassThrough(); child.stderr = new PassThrough();
    let buffered = ""; let request;
    child.stdin.on("data", (chunk) => {
      buffered += chunk;
      while (buffered.includes("\n")) {
        const index = buffered.indexOf("\n"); const line = buffered.slice(0, index); buffered = buffered.slice(index + 1);
        const frame = (value) => child.stdout.write(JSON.stringify({ schemaVersion: 1, id: request.id, ...value }) + "\n");
        if (!request) { request = JSON.parse(line); observe(request); frame({kind: "supervisor", processId: 10002, launcherProcessId: child.pid}); frame({ kind: "ready" }); }
        else if (line === "go") {
          mkdirSync(request.ownerDirectory);
          if (request.createNativeTestState) mkdirSync(request.environment.QC_REMOTE_TEST_STATE_ROOT);
          rmSync(request.ownerDirectory, { recursive: true });
          frame({ kind: "retirement", acknowledged: true, exitCode: 0, ownerDirectory: request.ownerDirectory });
          child.stdout.end(); child.stderr.end(); child.emit("close", 0, null);
        }
      }
    });
    return child;
  };
}

const here = dirname(fileURLToPath(import.meta.url));
const pwsh = process.platform === "win32" ? resolvePowerShell7() : null;

test("Node original admission proves exact native membership and rejects the unrelated observer", { skip: process.platform !== "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "qc-node-original-admission-"));
  const checker = join(here, "windows-owned-admission.ps1");
  let acknowledged = false, output = "", checked = false, errorText = "";
  const worker = `
    const {execFileSync}=require('node:child_process'),fs=require('node:fs');
    const [checker,pwsh,release,...args]=process.argv.slice(1);
    const at=args.indexOf('--original-job-admission');
    if(at<0)throw Error('No actual original admission');
    const admission=args[at+1];
    execFileSync(pwsh,['-NoProfile','-NonInteractive','-File',checker,'-AdmissionFile',admission,'-ExpectedParentProcessId',String(process.pid),'-ExpectedParentImagePath',process.execPath],{stdio:['ignore','pipe','pipe'],timeout:30000});
    process.stdout.write(JSON.stringify({admission})+'\\n');
    const clock=Date.now();const timer=setInterval(()=>{if(fs.existsSync(release)){clearInterval(timer);process.exit(0)}else if(Date.now()-clock>45000){throw Error('Observer did not finish')}} ,10);
  `;
  try {
    const receipt = await runOwnedWindowsCommand({ command: process.execPath,
      arguments: ["-e", worker, checker, pwsh, join(root, "release")], cwd: root,
      originalAdmission: true, timeoutMilliseconds: 120000, maxOutputBytes: 65536,
    }, { onStdout(chunk) {
      output += chunk;
      if (checked || !output.includes("\n")) return;
      const { admission } = JSON.parse(output.slice(0, output.indexOf("\n")));
      const foreign = spawnSync(pwsh, ["-NoProfile", "-NonInteractive", "-File", checker,
        "-AdmissionFile", admission, "-ExpectedParentProcessId", String(process.pid), "-ExpectedParentImagePath", process.execPath],
        { encoding: "utf8", timeout: 30000, windowsHide: true });
      assert.notEqual(foreign.status, 0, "knowing a real descriptor cannot admit a process outside the exact original Job");
      assert.match(String(foreign.stderr), /does not belong to the build Job/);
      checked = true;
      writeFileSync(join(root, "release"), "finish");
    }, onStderr(text) { errorText += text; } });
    acknowledged = receipt.retirementAcknowledged;
    assert.equal(receipt.code, 0, errorText);
    assert.equal(checked, true);
    assert.equal(acknowledged, true);
  } catch (error) { acknowledged ||= Boolean(error.retirementAcknowledged); throw error; }
  finally { if (acknowledged) await rm(root, { recursive: true, force: true }); else console.error("Unknown original retains Node admission fixture:", root); }
});

test("actual owned command receives stdin EOF rather than the original host protocol pipe", { skip: process.platform !== "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "qc-owned-git-stdin-"));
  let acknowledged = true;
  let stdout = "";
  let stderr = "";
  const sinks = { onStdout: (value) => { stdout += value; }, onStderr: (value) => { stderr += value; } };
  try {
    const canonicalRoot = await (await import("node:fs/promises")).realpath(root);
    // Prove actual EOF, not just a successful command that never reads stdin.
    // Setup and its Git child share the canonical original owner. The generous
    // fixture budget includes cold PowerShell/compiler startup, not just Git.
    acknowledged = false;
    const setup = await runOwnedWindowsCommand({
      command: process.execPath,
      arguments: ["-e", "let bytes=0;process.stdin.on('data',chunk=>bytes+=chunk.length);process.stdin.on('end',()=>{if(bytes!==0)throw Error('host protocol leaked to child stdin');const {spawnSync}=require('node:child_process');const p=spawnSync('git',['init','-q','-b','fixture'],{cwd:process.cwd(),stdio:['ignore','pipe','pipe'],encoding:'utf8',timeout:30000,maxBuffer:65536});if(p.status!==0)throw Error(String(p.error||p.stderr||p.status));process.stdout.write('QC_CHILD_STDIN_EOF\\n');});process.stdin.resume();"],
      cwd: root, environment: process.env, timeoutMilliseconds: 120_000, maxOutputBytes: 128 * 1024,
    }, sinks);
    acknowledged = setup.retirementAcknowledged;
    assert.equal(setup.code, 0, stdout + stderr);
    assert.equal(setup.retirementAcknowledged, true);
    assert.equal(stdout.trim(), "QC_CHILD_STDIN_EOF", "actual child observes empty stdin followed by EOF");
    stdout = ""; stderr = "";
    acknowledged = false;
    const result = await runOwnedWindowsCommand({
      command: "git", arguments: ["-C", root, "rev-parse", "--show-toplevel"],
      cwd: root, environment: process.env, timeoutMilliseconds: 120_000, maxOutputBytes: 64 * 1024,
    }, sinks);
    acknowledged = result.retirementAcknowledged;
    assert.equal(result.code, 0, stdout + stderr);
    assert.equal(stdout.trim().replaceAll("\\", "/").toLowerCase(), canonicalRoot.replaceAll("\\", "/").toLowerCase());
    assert.equal(acknowledged, true, "success includes exact original native/tree and PowerShell retirement");
  } catch (error) {
    acknowledged = acknowledged || Boolean(error.retirementAcknowledged);
    throw new Error(stdout + stderr, { cause: error });
  } finally {
    if (acknowledged) await rm(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    else console.error(`Unacknowledged original retains Git stdin fixture ${root}`);
  }
});

test("actual cooperative original stop refusal retains the job until a successful retry",
  { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-cooperative-job-stop-"));
    let acknowledged = false;
    try {
      const script = join(root, "cooperative-boundary.ps1");
      await writeFile(script, String.raw`param($Source,$Root)
$ErrorActionPreference='Stop'
$tokens=$null;$errors=$null
$ast=[Management.Automation.Language.Parser]::ParseFile($Source,[ref]$tokens,[ref]$errors)
if($errors.Count){throw 'Producer source must parse before transaction execution'}
foreach($name in @('Request-ProducerPowerShellStop','Complete-ProducerRetirement')){
  $nodes=@($ast.FindAll({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name},$true))
  if($nodes.Count -ne 1){throw "Missing unique real transaction $name"}
  Invoke-Expression $nodes[0].Extent.Text
}
$script:nativeCalls=0;$script:completed=0;$script:terminalBeforeNative=$false
# This controlled native ACK isolates the original-PS boundary. The separate
# actual descendant fixtures verify the real native Job; this fixture must
# not claim that its stub terminated any native process.
function Stop-QcOwnedProcessTree {
  param($OwnerFile,$TimeoutSeconds)
  $script:nativeCalls++
  if($script:nativeCalls -eq 2){$script:terminalBeforeNative=$job.State -in @('Completed','Stopped','Failed')}
  return $true
}
function Remove-QcBuildPowerShellJob {
  param($OwnerFile)
  if($job.State -notin @('Completed','Stopped','Failed')){throw 'Remove observed nonterminal original'}
  Microsoft.PowerShell.Core\Remove-Job -Job $job -ErrorAction Stop
}
function Complete-QcBuildPowerShellJobRetirement {
  param($OwnerFile)
  if($job.State -notin @('Completed','Stopped','Failed')){throw 'Complete observed nonterminal original'}
  if(@(Get-Job | Where-Object {$_.InstanceId -eq $job.InstanceId}).Count){throw 'Complete observed original still registered'}
  $script:completed++
}
$owner=Join-Path $Root 'controlled-original'
$job=$null;$retired=$false
try {
  $job=Start-Job {param($Signal) Write-Output 'ORIGINAL_READY';while(-not [IO.File]::Exists($Signal)){Start-Sleep -Milliseconds 10}} -ArgumentList "$owner.cancel"
  $ready=$false;$clock=[Diagnostics.Stopwatch]::StartNew()
  while(-not $ready){
    $ready=@(Receive-Job -Job $job | Where-Object {$_ -eq 'ORIGINAL_READY'}).Count -eq 1
    if(-not $ready -and ($clock.ElapsedMilliseconds -ge 30000 -or $job.State -ne 'Running')){throw 'Original did not reach controlled running state'}
    if(-not $ready){Start-Sleep -Milliseconds 10}
  }
  $actualRequest=(Get-Item Function:Request-ProducerPowerShellStop).ScriptBlock
  function Request-ProducerPowerShellStop {throw 'controlled signal publication refusal'}
  $failure=$null
  try {Complete-ProducerRetirement} catch {$failure=$_.Exception.Message}
  if($failure -notlike '*controlled signal publication refusal*'){throw "Missing exact signal fault: $failure"}
  if($script:nativeCalls -ne 1 -or $script:completed -ne 0){throw 'Signal failure skipped native cleanup or falsely completed retirement'}
  if($job.State -ne 'Running' -or @(Get-Job | Where-Object {$_.InstanceId -eq $job.InstanceId}).Count -ne 1){throw 'Signal failure lost actual original attribution'}
  Set-Item Function:Request-ProducerPowerShellStop -Value $actualRequest
  Complete-ProducerRetirement
  if($script:nativeCalls -ne 2 -or $script:completed -ne 1 -or -not $script:terminalBeforeNative){throw 'Cooperative retry did not finish original before native ACK and repository retirement'}
  if(@(Get-Job | Where-Object {$_.InstanceId -eq $job.InstanceId}).Count){throw 'Retry retained original repository object'}
  $retired=$true
  [Console]::Out.WriteLine('COOPERATIVE_STOP_RETRY_ACK')
} finally {
  if(-not $retired -and $null -ne $job){
    [IO.File]::WriteAllText("$owner.cancel",'fixture cleanup signal')
    if(-not (Wait-Job -Job $job -Timeout 3)){throw 'Fixture cooperative original cleanup unacknowledged; outer owner retained'}
    if(@(Get-Job | Where-Object {$_.InstanceId -eq $job.InstanceId}).Count){Remove-Job -Job $job -ErrorAction Stop}
  }
}
`);
      const environment = { ...process.env };
      for (const name of Object.keys(environment)) if (name.toUpperCase() === "PSMODULEPATH") delete environment[name];
      const result = spawnSync(pwsh, ["-NoLogo", "-NoProfile", "-NonInteractive", "-File", script,
        join(here, "windows-owned-producer.ps1"), root], {
        env: environment, encoding: "utf8", timeout: 45000, windowsHide: true,
      });
      acknowledged = result.status === 0 && result.stdout.includes("COOPERATIVE_STOP_RETRY_ACK");
      assert.equal(result.error, undefined, String(result.error ?? ""));
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.equal(acknowledged, true, result.stdout + result.stderr);
    } finally {
      if (acknowledged) await rm(root, { recursive: true, force: true });
      else console.error("Retained cooperative original fixture until outer owned-job retirement:", root);
    }
  });
const DRIVER = "param($Root,$Node,$Runner,$Producer,$Leaf,$Mode)\n$ErrorActionPreference='Stop'\n$original=$null\n$leafProcess=$null\n$leafVerified=$false\n$finished=$false\ntry {\n  $info=[Diagnostics.ProcessStartInfo]::new()\n  $info.FileName=$Node\n  $info.UseShellExecute=$false\n  $info.CreateNoWindow=$true\n  foreach($argument in @($Runner,$Root,$Node,$Producer,$Leaf,$Mode)){[void]$info.ArgumentList.Add($argument)}\n  $original=[Diagnostics.Process]::Start($info)\n  if($null -eq $original){throw 'Runner failed to start'}\n  $deadline=[DateTime]::UtcNow.AddSeconds(40)\n  $pidPath=Join-Path $Root 'leaf.pid'\n  while(-not [IO.File]::Exists($pidPath)){\n    if($original.HasExited -or [DateTime]::UtcNow -ge $deadline){throw 'Leaf never reached owned gate'}\n    Start-Sleep -Milliseconds 10\n  }\n  $identity=[IO.File]::ReadAllText($pidPath)|ConvertFrom-Json\n  $stateRoot=[string]$identity.testStateRoot\n  if([string]::IsNullOrWhiteSpace($stateRoot)){throw 'Native test state root was not supplied by existing owner'}\n  $diagnostic=Join-Path $stateRoot 'late-stderr.log'\n  while(-not [IO.File]::Exists($diagnostic)){\n    if($original.HasExited -or [DateTime]::UtcNow -ge $deadline){throw 'Owned descendant diagnostic never opened'}\n    Start-Sleep -Milliseconds 10\n  }\n  $leafPid=[int]$identity.processId\n  # Capture actual process object while the live gated leaf cannot exit/reuse.\n  $leafProcess=[Diagnostics.Process]::GetProcessById($leafPid)\n  $held=$leafProcess.Handle\n  if($leafProcess.StartTime.ToUniversalTime().Ticks -ne [long]$identity.createdAtTicks){throw 'Leaf process generation changed before capture'}\n  if($leafProcess.HasExited){throw 'Leaf exited before independent capture'}\n  $leafVerified=$true\n  [IO.File]::WriteAllText((Join-Path $Root 'release'),'captured original leaf')\n  if(-not $original.WaitForExit(90000)){throw 'Original runner retirement not acknowledged'}\n  if($original.ExitCode -ne 0){throw \"Runner exit $($original.ExitCode)\"}\n  if(-not $leafProcess.WaitForExit(1000)){throw 'Affirmative receipt left actual original descendant alive'}\n  $outcome=[IO.File]::ReadAllText((Join-Path $Root 'outcome.json'))|ConvertFrom-Json\n  if($outcome.retirementAcknowledged -ne $true){throw 'Missing affirmative retirement'}\n  if([IO.Directory]::Exists($stateRoot)){throw 'Acknowledged original/tree retirement left test state bytes'}\n  $finished=$true\n  [Console]::Out.WriteLine('OWNED_FIXTURE_ACK')\n} finally {\n  # Independent original leaf cleanup on unwind, never PID/image sweep.\n  if($null -ne $leafProcess){\n    if($leafVerified -and -not $leafProcess.HasExited){$leafProcess.Kill();[void]$leafProcess.WaitForExit(3000)}\n    $leafProcess.Dispose()\n  }\n  if($null -ne $original){$original.Dispose()}\n  # Unknown runner retains bytes; invoking canonical outer Job remains safety\n  # owner. Parent close alone is not claimed as descendant retirement proof.\n  if(-not $finished){[Console]::Error.WriteLine(\"Retained fixture attribution: $Root\")}\n}";

for (const { mode, completionPolicy } of [
  ...["root-exits-first", "cancel", "sink-failure", "inherited-output", "retirement-refusal"].map((mode) => ({ mode, completionPolicy: "bounded" })),
  ...["root-exits-first", "cancel", "inherited-output"].map((mode) => ({ mode, completionPolicy: "caller-lifetime" })),
]) {
  test("actual admitted producer retires its retained descendant: " + mode + " [" + completionPolicy + "]",
    { skip: process.platform !== "win32" }, async () => {
      assert.ok(existsSync(pwsh), "PowerShell7 prerequisite must exist before dispatch");
      const root = await mkdtemp(join(tmpdir(), "qc-owned-producer-behavior-"));
      let acknowledged = false;
      try {
        const leaf = join(root, "leaf.cjs");
        const producer = join(root, "producer.cjs");
        const launcher = join(root, "leaf-owner.ps1");
        const runner = join(root, "runner.mjs");
        const driver = join(root, "driver.ps1");
        let adapter = join(here, "windows-owned-producer.mjs");
        if (mode === "retirement-refusal") {
          const { mkdir } = await import("node:fs/promises");
          const copied = join(root, "copied-owner");
          await mkdir(copied);
          for (const name of ["windows-owned-producer.mjs", "windows-owned-supervisor-host.mjs", "windows-owned-producer.ps1", "windows-build-process-owner.ps1", "windows-build-process-cleanup.ps1"]) {
            let source = await readFile(join(here, name), "utf8");
            if (name === "windows-build-process-cleanup.ps1") source += "\nfunction Invoke-QcBuildJobRetirement { param($Owner,[int]$TimeoutMilliseconds) if (-not [IO.File]::Exists(" + "\'" + join(root, "allow-retirement").replaceAll("\'", "\'\'") + "\'" + ")) { throw \'controlled real retirement refusal\' }; $Owner.StopAndWait($TimeoutMilliseconds) }\n";
            await writeFile(join(copied, name), source);
          }
          adapter = join(copied, "windows-owned-producer.mjs");
        }
        await writeFile(leaf, [
          'const fs=require("node:fs"),path=require("node:path");',
          'const diagnostic=fs.openSync(path.join(process.env.QC_REMOTE_TEST_STATE_ROOT,"late-stderr.log"),"a");',
          'fs.writeSync(diagnostic,"owned descendant diagnostic\\n");',
          'setInterval(() => {fs.writeSync(diagnostic,"still alive\\n")}, 1000);',
        ].join("\n"));
        await writeFile(launcher, "param($Node,$Leaf,$Identity)\n$ErrorActionPreference='Stop'\n$info=[Diagnostics.ProcessStartInfo]::new()\n$info.FileName=$Node\n$info.UseShellExecute=$false\n$info.CreateNoWindow=$true\n[void]$info.ArgumentList.Add($Leaf)\n$original=[Diagnostics.Process]::Start($info)\ntry {\n  $held=$original.Handle\n  $record=@{processId=$original.Id;createdAtTicks=[string]$original.StartTime.ToUniversalTime().Ticks;testStateRoot=$env:QC_REMOTE_TEST_STATE_ROOT}|ConvertTo-Json -Compress\n  [IO.File]::WriteAllText(($Identity+'.stage'),$record)\n  [IO.File]::Move(($Identity+'.stage'),$Identity)\n  $original.WaitForExit()\n} finally {$original.Dispose()}\n");
        await writeFile(producer, [
          'const {spawn}=require("node:child_process"); const fs=require("node:fs");',
          'const child=spawn(process.argv[6],["-NoLogo","-NoProfile","-NonInteractive","-File",process.argv[7],process.execPath,process.argv[2],process.argv[3]],{stdio:process.argv[5]==="inherited-output"?"inherit":"ignore",windowsHide:true});',
          'child.unref();',
          'const deadline=Date.now()+30000;',
          'const timer=setInterval(()=>{if(Date.now()>deadline)process.exit(7);if(!fs.existsSync(process.argv[4]))return;',
          'clearInterval(timer);process.stdout.write("admitted\\n");',
          'if(["root-exits-first","inherited-output","retirement-refusal"].includes(process.argv[5]))process.exit(0);else setInterval(()=>{},1000);},10);',
        ].join("\n"));
        await writeFile(runner, [
          'import {writeFile,rename} from "node:fs/promises";',
          'import {runOwnedWindowsCommand,retainedWindowsProducerCount} from ' +
            JSON.stringify(pathToFileURL(adapter).href) + ';',
          'const [root,node,producer,leaf,mode]=process.argv.slice(2);',
          'const controller=new AbortController(); let outcome;',
          'try {',
          'try {outcome=await runOwnedWindowsCommand({command:node,arguments:[producer,leaf,root+"/leaf.pid",root+"/release",mode,process.env.QC_PWSH_EXECUTABLE,root+"/leaf-owner.ps1"],cwd:root,' + (completionPolicy === "bounded" ? 'timeoutMilliseconds:30000,' : 'completionPolicy:"caller-lifetime",') + 'nativeTestState:true},',
          '{signal:controller.signal,onStdout(){if(mode==="cancel")controller.abort();if(mode==="sink-failure")throw Error("fixture sink refused");}});',
          'if(mode!=="root-exits-first"||outcome.code!==0||outcome.retirementAcknowledged!==true)throw Error("unexpected successful outcome");',
          '} catch(error) {if(mode==="retirement-refusal"){if(error.retirementAcknowledged!==false)throw error;await writeFile(root+"/unknown.tmp",JSON.stringify({ownerDirectory:error.ownerDirectory,count:retainedWindowsProducerCount()}));await rename(root+"/unknown.tmp",root+"/unknown.json");const until=Date.now()+60000;while(retainedWindowsProducerCount()!==0){if(Date.now()>until)throw Error("late original never retired");await new Promise(r=>setTimeout(r,20));}outcome={retirementAcknowledged:true};}else{if(mode==="root-exits-first"||error.retirementAcknowledged!==true)throw error;outcome={retirementAcknowledged:true};}}',
          'if(retainedWindowsProducerCount()!==0)throw Error("original admission not retired");',
          'await writeFile(root+"/outcome.json",JSON.stringify(outcome));',
          '} catch(error) {await writeFile(root+"/failure.tmp",JSON.stringify({message:String(error?.message??error).slice(0,1024),stack:String(error?.stack??"").slice(0,2048),retirementAcknowledged:error?.retirementAcknowledged??null,ownerDirectory:typeof error?.ownerDirectory==="string"?error.ownerDirectory.slice(0,512):null,retainedCount:retainedWindowsProducerCount(),cause:String(error?.cause?.message??"").slice(0,512)}));await rename(root+"/failure.tmp",root+"/failure.json");throw error;}',
        ].join("\n"));
        const controlledDriver = DRIVER.replace("  if(-not $original.WaitForExit(90000))", [
          "  if($Mode -eq 'retirement-refusal'){",
          "    $unknown=Join-Path $Root 'unknown.json'",
          "    $unknownDeadline=[DateTime]::UtcNow.AddSeconds(45)",
          "    while(-not [IO.File]::Exists($unknown)){if($original.HasExited -or [DateTime]::UtcNow -ge $unknownDeadline){throw 'Negative receipt never arrived'};Start-Sleep -Milliseconds 10}",
          "    if($leafProcess.HasExited -or $original.HasExited){throw 'Unknown receipt did not retain actual original ownership'}",
          "    $state=[IO.File]::ReadAllText($unknown)|ConvertFrom-Json",
          "    if($state.count -ne 1 -or -not [IO.File]::Exists((Join-Path $state.ownerDirectory 'producer-owner.json'))){throw 'Unknown receipt lost admission or attribution'}",
          "    if(-not [IO.Directory]::Exists($stateRoot) -or -not [IO.File]::Exists($diagnostic)){throw 'Unknown native retirement lost descendant diagnostic root'}",
          "    [IO.File]::WriteAllText((Join-Path $Root 'allow-retirement'),'allow actual retained Job StopAndWait')",
          "  }",
          "  if(-not $original.WaitForExit(90000))",
        ].join("\n"));
        const exitCheck = '  if($original.ExitCode -ne 0){throw "Runner exit $($original.ExitCode)"}';
        assert.ok(controlledDriver.includes(exitCheck), "the actual driver exit check must be instrumented");
        const diagnosticDriver = controlledDriver.replace(exitCheck, [
          "  if($original.ExitCode -ne 0){",
          "    $detail='no bounded failure.json was published'",
          "    $failurePath=Join-Path $Root 'failure.json'",
          "    if([IO.File]::Exists($failurePath)){",
          "      $heldFailure=[IO.File]::OpenRead($failurePath)",
          "      try {",
          "        $bytes=New-Object byte[] 16384; $used=0",
          "        while($used -lt $bytes.Length){$count=$heldFailure.Read($bytes,$used,$bytes.Length-$used);if($count -eq 0){break};$used+=$count}",
          "        $detail=[Text.Encoding]::UTF8.GetString($bytes,0,$used)",
          "      } finally {$heldFailure.Dispose()}",
          "    }",
          '    throw "Runner exit $($original.ExitCode): $detail"',
          "  }",
        ].join("\n"));
        await writeFile(driver, diagnosticDriver);
        const environment = { ...process.env, QC_PWSH_EXECUTABLE: pwsh };
        delete environment.PSModulePath;
        const result = spawnSync(pwsh, [
          "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
          "-File", driver, root, process.execPath, runner, producer, leaf, mode,
        ], { encoding: "utf8", env: environment, timeout: 135000, windowsHide: true });
        acknowledged = result.status === 0 && result.stdout.includes("OWNED_FIXTURE_ACK");
        assert.equal(result.error, undefined, String(result.error ?? ""));
        assert.equal(result.status, 0, result.stdout + result.stderr);
        assert.equal(acknowledged, true, result.stdout + result.stderr);
      } finally {
        if (acknowledged) await rm(root, { recursive: true, force: true });
        else console.error("Retained exact fixture root until outer owned-job retirement:", root);
      }
    });
}


test("unknown producer originals consume admission until late affirmative retirement and original close",
  { skip: process.platform !== "win32" }, async () => {
    const originals = [];
    const fakeHost = () => {
      const child = Object.assign(new EventEmitter(), {pid: 10001});
      child.stdin = new PassThrough();
      child.stdout = new PassThrough();
      child.stderr = new PassThrough();
      let buffered = "";
      let request;
      child.writeFrame = (frame) => child.stdout.write(JSON.stringify({
        schemaVersion: 1, id: request.id, ...frame,
      }) + "\n");
      child.closeOriginal = () => {
        child.stdout.end(); child.stderr.end();
        child.emit("close", 1, null);
      };
      child.stdin.on("data", (chunk) => {
        buffered += chunk;
        while (buffered.includes("\n")) {
          const at = buffered.indexOf("\n");
          const line = buffered.slice(0, at);
          buffered = buffered.slice(at + 1);
          if (!request) {
            request = JSON.parse(line);
            child.request = request;
            child.stateExistedBeforeGo = existsSync(request.environment.QC_REMOTE_TEST_STATE_ROOT);
            child.writeFrame({kind: "supervisor", processId: 10002, launcherProcessId: child.pid});
            child.writeFrame({ kind: "ready" });
          } else if (line === "go") {
            mkdirSync(request.ownerDirectory);
            mkdirSync(request.environment.QC_REMOTE_TEST_STATE_ROOT);
            child.stateExistedAfterGo = existsSync(request.environment.QC_REMOTE_TEST_STATE_ROOT);
            child.writeFrame({ kind: "retirement", acknowledged: false,
              ownerDirectory: request.ownerDirectory, error: "controlled unknown original" });
          }
        }
      });
      child.retire = () => {
        rmSync(request.ownerDirectory, { recursive: true });
        child.writeFrame({ kind: "retirement", acknowledged: true,
          ownerDirectory: request.ownerDirectory, exitCode: 1, error: "controlled earlier failure" });
        child.closeOriginal();
      };
      originals.push(child);
      return child;
    };
    try {
      for (let index = 0; index < 4; index++) {
        await assert.rejects(runOwnedWindowsCommand({
          command: process.execPath, arguments: [], cwd: process.cwd(), nativeTestState: true,
          environment: { ...process.env, QC_REMOTE_TEST_STATE_ROOT: "foreign-unowned-state",
            qc_remote_test_state_root: "foreign-case-state" },
        }, { executable: process.execPath, spawnHost: fakeHost }), (error) =>
          error.retirementAcknowledged === false && /unacknowledged/.test(error.message));
      }
      assert.equal(retainedWindowsProducerCount(), 4);
      for (const child of originals) {
        assert.equal(child.stateExistedBeforeGo, false, "Node caller does not create host-owned bytes");
        assert.equal(child.stateExistedAfterGo, true, "host owns state before command admission");
        assert.equal(child.request.environment.QC_REMOTE_TEST_STATE_ROOT,
          join(child.request.ownerDirectory, "native-test-state"));
        assert.equal(child.request.environment.qc_remote_test_state_root, undefined,
          "ambient case variants do not acquire deletion authority");
        assert.equal(existsSync(child.request.environment.QC_REMOTE_TEST_STATE_ROOT), true,
          "negative native receipt retains diagnostic state with original attribution");
      }
      await assert.rejects(runOwnedWindowsCommand({
        command: process.execPath, arguments: [], cwd: process.cwd(),
      }, { executable: process.execPath, spawnHost: () => { throw Error("capacity must refuse before spawn"); } }),
      /headroom exhausted/);
    } finally {
      // These protocol doubles never launched a native process; emit late
      // receipts and close every exact original so real owned temp bytes retire.
      for (const child of originals) child.retire();
      await reapRetiredWindowsProducers();
    }
    assert.equal(retainedWindowsProducerCount(), 0);
    for (const child of originals) {
      assert.equal(existsSync(child.request.environment.QC_REMOTE_TEST_STATE_ROOT), false,
        "only late affirmative retirement and exact original close remove owned state");
    }
  });


test("absolute mixed-case PATH discovers a nonstandard real PowerShell7 install",
  { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-pwsh-discovery-"));
    let acknowledged = false;
    try {
      const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
        !["PATH", "PROGRAMFILES", "QC_PWSH_EXECUTABLE"].includes(name.toUpperCase())));
      environment.pAtH = 'relative-must-not-be-searched;"' + dirname(pwsh) + '"';
      environment.pRoGrAmFiLeS = join(root, "no-standard-install");
      assert.equal(resolvePowerShell7(environment), pwsh);
      let output = "";
      const result = await runOwnedWindowsCommand({
        command: process.execPath, arguments: ["-e", "process.stdout.write('alternate PATH admitted')"],
        cwd: root, environment, timeoutMilliseconds: 30000,
      }, { onStdout(chunk) { output += chunk; } });
      acknowledged = result.retirementAcknowledged === true;
      assert.deepEqual(result, { code: 0, retirementAcknowledged: true });
      assert.equal(output, "alternate PATH admitted");
    } finally {
      // No producer writes into this fixture directory; owned process bytes
      // belong to the adapter's separately retained original owner.
      if (acknowledged) await rm(root, { recursive: true, force: true });
      else console.error("Retained nonstandard-host fixture until original retirement:", root);
    }
  });

test("missing or relative-only PowerShell PATH refuses before host or producer spawn",
  { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-pwsh-missing-"));
    let spawns = 0;
    try {
      const environment = { pAtH: ".;relative-only", pRoGrAmFiLeS: join(root, "not-installed") };
      await assert.rejects(runOwnedWindowsCommand({
        command: process.execPath, arguments: ["-e", "throw Error('must never dispatch')"],
        cwd: root, environment,
      }, { spawnHost() { spawns++; throw Error("must refuse before host spawn"); } }), /requires PowerShell7/);
      assert.equal(spawns, 0);
      assert.equal(retainedWindowsProducerCount(), 0);
      assert.throws(() => resolvePowerShell7({ ...environment, qc_pwsh_executable: "relative/pwsh.exe" }),
        /existing absolute/);
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  });


test("WindowsPowerShell5 is refused before any producer dispatch",
  { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-owned-producer-old-host-"));
    let acknowledged = false;
    try {
      const marker = join(root, "must-not-dispatch");
      const oldHost = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
      assert.ok(existsSync(oldHost));
      await assert.rejects(runOwnedWindowsCommand({
        command: process.execPath,
        arguments: ["-e", "require('node:fs').writeFileSync(process.argv[1],'unsafe')", marker],
        cwd: root,
      }, { executable: oldHost }), (error) => {
        acknowledged = error.retirementAcknowledged === true;
        return acknowledged && /PowerShell|bootstrap/i.test(error.message);
      });
      assert.equal(existsSync(marker), false);
      assert.equal(retainedWindowsProducerCount(), 0);
    } finally {
      if (acknowledged) await rm(root, { recursive: true, force: true });
      else console.error("Retained original bootstrap fixture:", root);
    }
  });



test("actual host emits ready while go is delayed, then retires on pre-go EOF",
  { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-owned-bootstrap-eof-"));
    let child;
    let closed = false;
    let closeResult;
    let stderr = "";
    let buffer = "";
    let readyResolve;
    let readyReject;
    const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
    ready.catch(() => {}); // Setup failure must not leave an unobserved event rejection.
    const waitWithin = async (promise, milliseconds) => {
      let timer;
      try {
        return await Promise.race([promise, new Promise((_, reject) => {
          timer = setTimeout(() => reject(Error("Owned bootstrap fixture deadline expired")), milliseconds);
        })]);
      } finally { clearTimeout(timer); }
    };
    let originalClose;
    try {
      const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
        name.toUpperCase() !== "PSMODULEPATH"));
      child = spawn(pwsh, ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
        "-File", join(here, "windows-owned-producer.ps1")],
      { env: environment, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
      originalClose = new Promise((resolve) => child.once("close", (code, signal) => {
        closed = true;
        closeResult = { code, signal };
        readyReject(Error("Original bootstrap closed before ready: " + stderr));
        resolve(closeResult);
      }));
      child.once("error", readyReject);
      child.stdin.on("error", readyReject);
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (chunk) => { stderr += chunk.slice(0, Math.max(0, 32768 - stderr.length)); });
      child.stdout.setEncoding("utf8");
      child.stdout.on("data", (chunk) => {
        try {
          buffer += chunk;
          if (buffer.length > 65536) throw Error("Unexpected bootstrap output envelope");
          while (buffer.includes("\n")) {
            const at = buffer.indexOf("\n");
            const frame = JSON.parse(buffer.slice(0, at));
            buffer = buffer.slice(at + 1);
            assert.equal(frame.id, "actual-delayed-go-eof");
            assert.equal(frame.kind, "ready");
            readyResolve(frame);
          }
        } catch (error) { readyReject(error); }
      });
      child.stdin.write(JSON.stringify({
        schemaVersion: 1, id: "actual-delayed-go-eof",
        command: process.execPath, arguments: ["-e", "throw Error('must not dispatch')"],
        cwd: root, environment, ownerDirectory: root,
        timeoutMilliseconds: 30000, maxOutputBytes: 65536,
      }) + "\n");
      await waitWithin(ready, 45000);
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.equal(closed, false, "original host must remain alive awaiting go");
      assert.deepEqual(await readdir(root), [], "no native admission or producer before explicit go");
      child.stdin.end();
      await waitWithin(originalClose, 5000);
      assert.equal(closeResult.code, 1, "EOF must explicitly refuse admission rather than execute producer");
      assert.equal(closeResult.signal, null, "external termination is not an EOF refusal");
      assert.match(stderr, /Producer bootstrap input closed before dispatch/);
      assert.deepEqual(await readdir(root), [], "pre-go EOF leaves no admission bytes");
    } finally {
      // This fixture NEVER sends go. The trusted PS7 bootstrap has no external
      // compiler or producer child; only this original host may be killed.
      if (child && !closed) {
        child.kill();
        try { await waitWithin(originalClose, 3000); } catch {}
      }
      if (!child || closed) await rm(root, { recursive: true, force: true });
      else console.error("Retained unacknowledged pre-go original fixture:", root);
    }
  });

test("producer request envelope counts UTF-8 bytes before host spawn",
  { skip: process.platform !== "win32" }, async () => {
    let spawns = 0;
    await assert.rejects(runOwnedWindowsCommand({
      command: process.execPath, arguments: ["Ω".repeat(70000)], cwd: process.cwd(),
    }, { spawnHost() { spawns++; throw Error("must not dispatch over-budget bytes"); } }),
    /128KiB UTF-8/);
    assert.equal(spawns, 0);
    assert.equal(retainedWindowsProducerCount(), 0);
  });
test("actual PowerShell7 producer preserves literal argv and environment values",
  { skip: process.platform !== "win32" }, async () => {
    const argumentsToPreserve = ["space value", 'quote"value\\', "line\nvalue", "2026-10-14T12:34:56.789Z", "Ω🎸"];
    const value = '2026-10-14T12:34:56.789Z "quoted"\\path\nnext line';
    let output = "";
    const result = await runOwnedWindowsCommand({
      command: process.execPath,
      arguments: ["-e", "process.stdout.write(JSON.stringify({argv:process.argv.slice(1),value:process.env.QC_LITERAL_OWNER_FIXTURE}))", ...argumentsToPreserve],
      cwd: process.cwd(),
      environment: { ...process.env, QC_LITERAL_OWNER_FIXTURE: value },
      timeoutMilliseconds: 30000,
    }, { onStdout(chunk) { output += chunk; } });
    assert.deepEqual(result, { code: 0, retirementAcknowledged: true });
    assert.deepEqual(JSON.parse(output), { argv: argumentsToPreserve, value });
    assert.equal(retainedWindowsProducerCount(), 0);
  });

test("caller-lifetime policy is explicit and does not change bounded defaults", { skip: process.platform !== "win32" }, async () => {
  for (const policy of ["bounded", "caller-lifetime"]) {
    let request;
    const fakeHost = () => {
      const child = Object.assign(new EventEmitter(), {pid: 10001});
      child.stdin = new PassThrough(); child.stdout = new PassThrough(); child.stderr = new PassThrough();
      let buffered = "";
      child.stdin.on("data", (chunk) => {
        buffered += chunk;
        while (buffered.includes("\n")) {
          const index = buffered.indexOf("\n");
          const line = buffered.slice(0, index); buffered = buffered.slice(index + 1);
          const frame = (value) => child.stdout.write(JSON.stringify({ schemaVersion: 1, id: request.id, ...value }) + "\n");
          if (!request) { request = JSON.parse(line); frame({kind: "supervisor", processId: 10002, launcherProcessId: child.pid}); frame({ kind: "ready" }); }
          else if (line === "go") {
            frame({ kind: "retirement", acknowledged: true, exitCode: 0, ownerDirectory: request.ownerDirectory });
            child.stdout.end(); child.stderr.end(); child.emit("close", 0, null);
          }
        }
      });
      return child;
    };
    assert.deepEqual(await runOwnedWindowsCommand({
      command: process.execPath, arguments: [], cwd: process.cwd(), completionPolicy: policy,
    }, { executable: process.execPath, spawnHost: fakeHost }), { code: 0, retirementAcknowledged: true });
    assert.equal(request.completionPolicy, policy);
    assert.equal(request.timeoutMilliseconds, policy === "bounded" ? 3_600_000 : null);
    assert.equal(request.maxOutputBytes, policy === "bounded" ? 67_108_864 : null);
    assert.equal(existsSync(request.ownerDirectory), false);
    assert.equal(retainedWindowsProducerCount(), 0);
  }
});

test("caller-lifetime rejects ambiguous budgets and unknown policies before admission", { skip: process.platform !== "win32" }, async () => {
  let spawns = 0;
  for (const options of [
    { completionPolicy: "caller-lifetime", timeoutMilliseconds: 10 },
    { completionPolicy: "caller-lifetime", maxOutputBytes: 32 },
    { completionPolicy: "unknown" },
  ]) {
    await assert.rejects(runOwnedWindowsCommand({ command: process.execPath, arguments: [], cwd: process.cwd(), ...options }, {
      executable: process.execPath, spawnHost() { spawns++; throw Error("must refuse before admission"); },
    }), /completion policy|cannot also declare/);
  }
  assert.equal(spawns, 0); assert.equal(retainedWindowsProducerCount(), 0);
});

test("an enclosing creation namespace grants authority only over the newly created child", { skip: process.platform !== "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "qc-owned-parent-scope-"));
  try {
    const sentinel = join(root, "sentinel.txt"); await writeFile(sentinel, "existing caller data");
    let cases = 0;
    for (const explicit of [false, true]) {
      let request;
      const result = await runOwnedWindowsCommand({
        command: process.execPath, arguments: [], cwd: process.cwd(),
        environment: { ...process.env, QC_VALIDATION_PRODUCER_ROOT: root },
        ...(explicit ? { ownerParentDirectory: root } : {}),
      }, { executable: process.execPath, spawnHost: successfulProtocolHost((value) => { request = value; }) });
      assert.deepEqual(result, { code: 0, retirementAcknowledged: true });
      assert.equal(dirname(request.ownerDirectory), root);
      assert.ok(request.ownerDirectory.startsWith(join(root, "qc-owned-producer-")));
      assert.equal(existsSync(request.ownerDirectory), false);
      assert.equal(await readFile(sentinel, "utf8"), "existing caller data");
      assert.deepEqual(await readdir(root), ["sentinel.txt"]);
      cases++;
    }
    assert.equal(cases, 2); assert.equal(retainedWindowsProducerCount(), 0);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("relative and junction creation namespaces refuse before host spawn", { skip: process.platform !== "win32" }, async () => {
  const root = await mkdtemp(join(tmpdir(), "qc-owned-parent-refusal-"));
  let spawns = 0;
  try {
    const { mkdir, symlink } = await import("node:fs/promises");
    const target = join(root, "target"); const alias = join(root, "alias");
    await mkdir(target); await symlink(target, alias, "junction");
    let cases = 0;
    for (const creationParent of ["relative-scope", alias]) {
      await assert.rejects(runOwnedWindowsCommand({
        command: process.execPath, arguments: [], cwd: root,
        environment: { ...process.env, QC_VALIDATION_PRODUCER_ROOT: creationParent },
      }, { executable: process.execPath, spawnHost() { spawns++; throw Error("must refuse before spawn"); } }), /creation namespace/);
      cases++;
    }
    assert.equal(cases, 2); assert.equal(spawns, 0); assert.equal(retainedWindowsProducerCount(), 0);
    assert.deepEqual(await readdir(target), []);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("actual caller-lifetime commands preserve streamed output and nonzero outcomes", { skip: process.platform !== "win32" }, async () => {
  for (const code of [0, 17]) {
    let stdout = "";
    const result = await runOwnedWindowsCommand({
      command: process.execPath, arguments: ["-e", "process.stdout.write('x'.repeat(131072),()=>process.exit(Number(process.argv[1])))", String(code)],
      cwd: process.cwd(), completionPolicy: "caller-lifetime",
    }, { onStdout(value) { stdout += value; } });
    assert.deepEqual(result, { code, retirementAcknowledged: true });
    assert.equal(stdout, "x".repeat(131072)); assert.equal(retainedWindowsProducerCount(), 0);
  }
});

for (const fixture of [
  { allow: false, createDirectory: false }, { allow: true, createDirectory: false },
  { allow: false, createDirectory: true }, { allow: true, createDirectory: true },
  { allow: false, createDirectory: true, preexisting: true },
]) {
  const { allow, createDirectory, preexisting = false } = fixture;
  test("actual original Job observer gates dispatch: " + (preexisting ? "refuse existing staging" : allow ? "acknowledge" : "refuse") + (createDirectory ? " [host-created staging]" : " [caller staging]"), { skip: process.platform !== "win32" }, async () => {
    const root = await mkdtemp(join(tmpdir(), "qc-owned-observer-"));
    const marker = join(root, "command-started");
    const ownerDirectory = createDirectory && !preexisting ? join(root, "host-created") : root;
    const sentinel = join(root, 'sentinel.txt');
    let child; let closed = false; let acknowledged = false; let retirement;
    let ownership; let stderr = ""; let buffer = "";
    let resolveOwnership; let rejectOwnership;
    const ownershipReady = new Promise((resolve, reject) => { resolveOwnership = resolve; rejectOwnership = reject; });
    ownershipReady.catch(() => {});
    const waitWithin = async (promise, milliseconds) => {
      let timer;
      try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Error("Observer fixture deadline expired")), milliseconds); })]); }
      finally { clearTimeout(timer); }
    };
    let originalClose;
    try {
      await writeFile(sentinel, 'existing caller bytes');
      const environment = Object.fromEntries(Object.entries(process.env).filter(([name]) => name.toUpperCase() !== "PSMODULEPATH"));
      child = spawn(pwsh, ["-NoProfile", "-NonInteractive", "-File", join(here, "windows-owned-producer.ps1")], {
        env: environment, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
      });
      originalClose = new Promise((resolve) => child.once("close", (code, signal) => {
        closed = true; rejectOwnership(Error("Host closed before observer: " + stderr)); resolve({ code, signal });
      }));
      child.once("error", rejectOwnership); child.stdin.on("error", rejectOwnership);
      child.stderr.setEncoding("utf8"); child.stderr.on("data", (chunk) => { stderr += chunk.slice(0, Math.max(0, 32768 - stderr.length)); });
      child.stdout.setEncoding("utf8"); child.stdout.on("data", (chunk) => {
        try {
          buffer += chunk;
          if (buffer.length > 65536) throw Error("Observer frame exceeded fixture envelope");
          while (buffer.includes("\n")) {
            const index = buffer.indexOf("\n"); const frame = JSON.parse(buffer.slice(0, index)); buffer = buffer.slice(index + 1);
            assert.equal(frame.id, "actual-job-observer");
            if (frame.kind === "ready") child.stdin.write("go\n");
            else if (frame.kind === "ownership") { ownership = frame; resolveOwnership(frame); }
            else if (frame.kind === "retirement") { retirement = frame; acknowledged = frame.acknowledged === true; }
          }
        } catch (error) { rejectOwnership(error); }
      });
      child.stdin.write(JSON.stringify({
        schemaVersion: 1, id: "actual-job-observer", observeOriginalJob: true, createOwnerDirectory: createDirectory,
        command: process.execPath, arguments: ["-e", "require('node:fs').writeFileSync(process.argv[1],'started')", marker],
        cwd: root, environment, ownerDirectory, timeoutMilliseconds: 30000, maxOutputBytes: 65536,
      }) + "\n");
      if (preexisting) {
        const exit = await waitWithin(originalClose, 45000);
        assert.equal(exit.code, 1, stderr + JSON.stringify(retirement));
        assert.equal(acknowledged, true); assert.equal(ownership, undefined);
        assert.equal(existsSync(marker), false, 'existing staging must refuse before command dispatch');
        assert.equal(await readFile(sentinel, 'utf8'), 'existing caller bytes');
        return;
      }
      await waitWithin(ownershipReady, 45000);
      assert.equal(ownership.parentProcessId, child.pid);
      assert.ok(Number.isInteger(ownership.processId) && ownership.processId > 0);
      assert.match(ownership.createdAtFileTime, /^\d+$/); assert.ok(ownership.jobName.startsWith("Local\\QC-build-owner-"));
      await new Promise((resolve) => setTimeout(resolve, 100));
      assert.equal(existsSync(marker), false, "retained host Job alone must not bypass its enclosing observer");
      child.stdin.write(allow ? "owned\n" : "cancel\n");
      const exit = await waitWithin(originalClose, 45000);
      assert.equal(exit.code, allow ? 0 : 1, stderr + JSON.stringify(retirement));
      assert.equal(exit.signal, null); assert.equal(acknowledged, true);
      assert.equal(existsSync(marker), allow);
      assert.equal(await readFile(sentinel, 'utf8'), 'existing caller bytes');
      if (createDirectory) assert.equal(existsSync(ownerDirectory), false, 'actual host retires only its exclusive staging');
      if (!allow) assert.match(retirement.error, /observer refused command admission/);
    } finally {
      if (child && !closed) { child.stdin.end("cancel\n"); try { await waitWithin(originalClose, 10000); } catch {} }
      if (closed && acknowledged) await rm(root, { recursive: true, force: true });
      else console.error("Retained observer fixture until outer original-tree acknowledgement:", root);
    }
  });
}
