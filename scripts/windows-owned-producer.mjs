import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { existsSync, realpathSync, writeFileSync } from "node:fs";
import { lstat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";

const HOST = join(dirname(fileURLToPath(import.meta.url)), "windows-owned-producer.ps1");
const LAUNCHER = join(dirname(fileURLToPath(import.meta.url)), "windows-owned-supervisor-host.mjs");
const ORIGINALS = new Set();
const MAX_ORIGINALS = 4;
const STARTUP_MS = 45_000;
const CLEANUP_MS = 45_000;
const LINE_LIMIT = 65_536;
const STDERR_LIMIT = 32_768;

export function retainedWindowsProducerCount() { return ORIGINALS.size; }
export async function reapRetiredWindowsProducers() {
  for (const owner of ORIGINALS) await owner.reap?.();
}
function windowsEnvironmentValue(environment, name) {
  // Match Node's Windows child-env rule when a plain object contains duplicate
  // case variants: first lexicographic key wins, not insertion order.
  const key = Object.keys(environment).filter((key) => key.toUpperCase() === name.toUpperCase()).sort()[0];
  return key === undefined ? undefined : environment[key];
}
export function resolvePowerShell7(environment = process.env) {
  const explicit = windowsEnvironmentValue(environment, "QC_PWSH_EXECUTABLE");
  if (explicit !== undefined && explicit !== null) {
    if (typeof explicit === "string" && isAbsolute(explicit) && existsSync(explicit)) return explicit;
    throw new Error("QC_PWSH_EXECUTABLE must name an existing absolute PowerShell7 executable before Cargo dispatch.");
  }
  const entries = String(windowsEnvironmentValue(environment, "PATH") ?? "").split(";");
  for (let entry of entries) {
    entry = entry.trim();
    if (entry.startsWith('"') && entry.endsWith('"')) entry = entry.slice(1, -1);
    // Never search CWD or interpret an inherited relative PATH component.
    if (!isAbsolute(entry)) continue;
    const candidate = join(entry, "pwsh.exe");
    if (existsSync(candidate)) return candidate;
  }
  const candidate = join(windowsEnvironmentValue(environment, "ProgramFiles") ?? "C:\\Program Files", "PowerShell", "7", "pwsh.exe");
  if (isAbsolute(candidate) && existsSync(candidate)) return candidate;
  throw new Error("Standalone Windows native validation requires PowerShell7 (pwsh.exe) on an absolute PATH entry or QC_PWSH_EXECUTABLE before Cargo dispatch.");
}
function ownershipError(message, owner, cause) {
  const error = new Error(message, cause ? { cause } : undefined);
  error.retirementAcknowledged = false;
  error.ownerDirectory = owner.directory;
  return error;
}

// Exact host/attribution survive affirmative native+PowerShell retirement,
// original host close and own-byte cleanup. No PID/taskkill fallback.
export async function runOwnedWindowsCommand({
  command, arguments: argv, cwd, environment = process.env,
  timeoutMilliseconds, maxOutputBytes, completionPolicy = "bounded",
  nativeTestState = false, ownerParentDirectory, originalAdmission = false,
}, {
  signal, onStdout = () => {}, onStderr = () => {}, spawnHost = spawn,
  executable = resolvePowerShell7(environment),
} = {}) {
  if (process.platform !== "win32") throw new Error("Owned Windows producers require Windows.");
  if (!["bounded", "caller-lifetime"].includes(completionPolicy)) throw new Error("Unknown Windows producer completion policy.");
  const followsCaller = completionPolicy === "caller-lifetime";
  if (followsCaller && (timeoutMilliseconds !== undefined || maxOutputBytes !== undefined)) {
    throw new Error("Caller-lifetime producers cannot also declare bounded execution/output limits.");
  }
  const executionLimit = followsCaller ? null : (timeoutMilliseconds ?? 3_600_000);
  const outputLimit = followsCaller ? null : (maxOutputBytes ?? 67_108_864);
  if (typeof nativeTestState !== "boolean") throw new Error("Native test-state ownership must be an explicit boolean option.");
  if (typeof originalAdmission !== "boolean") throw new Error("Original Job admission must be an explicit boolean option.");
  await reapRetiredWindowsProducers(); // No native owner locks across byte cleanup.
  if (ORIGINALS.size >= MAX_ORIGINALS) throw new Error("Previous Windows producers are still retiring; original-owner headroom exhausted.");
  if (signal?.aborted) throw new Error("Windows producer cancelled before admission.");
  const owner = { child: null, directory: null, go: false, closed: false, acknowledged: false, cleaning: null, stopping: false };
  ORIGINALS.add(owner);
  let finish;
  let fail;
  let settled = false;
  let timer;
  let text = "";
  let stderr = "";
  let failure = null;
  let receipt = null;
  const result = new Promise((resolve, reject) => { finish = resolve; fail = reject; });
  const reject = (error) => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    fail(error);
  };
  const retireBytes = () => {
    if (!owner.closed || !owner.acknowledged || owner.cleaning) return;
    owner.cleaning = (async () => {
      try {
        // The surviving PowerShell host exclusively creates and retires its
        // staging. The Node caller must not delete by pathname, particularly
        // when host creation refused an existing directory.
        ORIGINALS.delete(owner);
        if (!settled) {
          settled = true;
          clearTimeout(timer);
          if (failure) { failure.retirementAcknowledged = true; fail(failure); }
          else finish({ code: receipt.exitCode, retirementAcknowledged: true });
        }
      } catch (error) {
        owner.cleaning = null; // Keep exact attribution, allow later admission to retry.
        reject(ownershipError("Owned producer bytes did not retire; original attribution retained.", owner, error));
      }
    })();
  };
  owner.reap = async () => {
    retireBytes();
    if (owner.cleaning) await owner.cleaning;
  };
  const stop = (error) => {
    failure ??= error;
    if (owner.stopping) return; // Never extend a common cleanup deadline.
    owner.stopping = true;
    if (!owner.child) return;
    if (!owner.go) {
      // No go was attempted: close the bootstrap protocol, but retain the
      // lifetime launcher until its exact PS child closes. Killing only the
      // launcher would skip that observation, even before command admission.
      try { owner.child.stdin.end("cancel\n"); } catch {}
    } else {
      // Continue draining after sink failure so host cleanup can progress.
      try { owner.child.stdin.end("cancel\n"); } catch {}
    }
    clearTimeout(timer);
    timer = setTimeout(() => reject(ownershipError("Windows producer retirement remains unacknowledged.", owner, failure)), CLEANUP_MS);
  };
  const abort = () => stop(new Error("Windows producer cancelled; no automatic replay."));
  try {
    // An inherited namespace is creation scope, never authority to remove its
    // parent or existing files. The surviving host creates its exclusive child.
    const creationParent = ownerParentDirectory ?? windowsEnvironmentValue(environment, "QC_VALIDATION_PRODUCER_ROOT");
    if (creationParent !== undefined) {
      if (typeof creationParent !== "string" || !isAbsolute(creationParent)) throw new Error("Owned producer creation namespace must be an absolute directory.");
      const parentInfo = await lstat(creationParent);
      if (!parentInfo.isDirectory() || parentInfo.isSymbolicLink()) throw new Error("Owned producer creation namespace must be a plain directory.");
    }
    // TEMP may be an 8.3 short path (C:\Users\RUNNER~1\...). The admission
    // checker requires its file path to equal [IO.Path]::GetFullPath of
    // itself, which expands short names, so derive the default from the
    // long (native real) path.
    owner.directory = join(creationParent ?? realpathSync.native(tmpdir()), `qc-owned-producer-${randomUUID()}`);
    const childEnvironment = { ...environment };
    // An inherited path is not deletion authority. This producer derives its
    // state exclusively beneath the SAME original owner's admitted directory.
    for (const key of Object.keys(childEnvironment)) {
      if (key.toUpperCase() === "QC_REMOTE_TEST_STATE_ROOT") delete childEnvironment[key];
    }
    if (nativeTestState) {
      const testState = join(owner.directory, "native-test-state");
      childEnvironment.QC_REMOTE_TEST_STATE_ROOT = testState;
    }
    if (signal?.aborted) throw new Error("Windows producer cancelled before spawn.");
    const id = randomUUID();
    const admissionPath = originalAdmission ? join(owner.directory, "original-job-admission.json") : null;
    const request = { schemaVersion: 1, id, command,
      arguments: originalAdmission ? [...argv, "--original-job-admission", admissionPath] : argv,
      ...(originalAdmission ? { observeOriginalJob: true } : {}), cwd, environment: childEnvironment,
      ownerDirectory: owner.directory, createOwnerDirectory: true, createNativeTestState: nativeTestState,
      completionPolicy, timeoutMilliseconds: executionLimit, maxOutputBytes: outputLimit };
    const requestText = JSON.stringify(request);
    if (Buffer.byteLength(requestText, "utf8") > 131_072) throw new Error("Windows producer request exceeds its 128KiB UTF-8 software envelope.");
    const childEnv = { ...environment };
    for (const key of Object.keys(childEnv)) {
      if (key.toUpperCase() === "PSMODULEPATH") delete childEnv[key];
    }
    owner.child = spawnHost(process.execPath, [LAUNCHER, executable, HOST, id], {
      // Keep this cleanup supervisor alive when the Node launcher dies. It
      // remains observed through this exact ChildProcess and protocol pipes;
      // detached is lifetime isolation, not unref/fire-and-forget dispatch.
      env: childEnv, detached: true, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    });
    const handleFrame = (frame) => {
      if (!frame || frame.schemaVersion !== 1 || frame.id !== id) throw new Error("Invalid Windows producer protocol identity.");
      if (frame.kind === "supervisor") {
        if (owner.supervisorPid || owner.go || receipt || !Number.isSafeInteger(frame.processId) || frame.processId <= 0 || frame.launcherProcessId !== owner.child.pid) throw new Error("Invalid supervisor identity frame.");
        owner.supervisorPid = frame.processId;
      } else if (frame.kind === "ready") {
        if (!owner.supervisorPid) throw new Error("Producer admission preceded its original supervisor identity.");
        if (failure || signal?.aborted) return; // No late go after predispatch cancellation.
        if (owner.go || receipt) throw new Error("Duplicate producer admission.");
        owner.go = true;
        owner.child.stdin.write("go\n");
        clearTimeout(timer);
        // Foreground validation previously had no wall-clock/output ceiling.
        // Caller EOF/cancellation still retires the original owned tree.
        if (!followsCaller) timer = setTimeout(() => stop(new Error("Windows producer whole invocation deadline expired.")), executionLimit);
      } else if (frame.kind === "ownership") {
        if (!originalAdmission || !owner.go || owner.originalObserved || receipt || failure || signal?.aborted ||
            !Number.isSafeInteger(frame.processId) || frame.processId <= 0 || frame.parentProcessId !== owner.supervisorPid ||
            typeof frame.imagePath !== "string" || realpathSync.native(frame.imagePath).toLowerCase() !== realpathSync.native(executable).toLowerCase() ||
            typeof frame.createdAtFileTime !== "string" || !/^[1-9][0-9]{0,18}$/.test(frame.createdAtFileTime) ||
            typeof frame.jobName !== "string" || !frame.jobName.startsWith(`Local\\QC-build-owner-${frame.processId}-`) ||
            !/^[0-9a-f]{32}$/.test(frame.jobName.slice(`Local\\QC-build-owner-${frame.processId}-`.length))) {
          throw new Error("Invalid original Job admission frame.");
        }
        // The native checker opens this exact Job/root identity and verifies
        // membership. A path/flag alone is never worker admission authority.
        const descriptor = JSON.stringify({ schemaVersion: 1, jobName: frame.jobName, processId: frame.processId,
          createdAtFileTime: frame.createdAtFileTime, parentProcessId: frame.parentProcessId, imagePath: frame.imagePath });
        if (Buffer.byteLength(descriptor, "utf8") > 4096) throw new Error("Original admission descriptor exceeds its envelope.");
        writeFileSync(admissionPath, descriptor, { flag: "wx", mode: 0o600 });
        owner.originalObserved = true;
        owner.child.stdin.write("owned\n");
      } else if (frame.kind === "stdout" || frame.kind === "stderr") {
        if (!owner.go || (originalAdmission && !owner.originalObserved) || typeof frame.text !== "string") throw new Error("Invalid producer output frame.");
        if (!failure) (frame.kind === "stdout" ? onStdout : onStderr)(frame.text);
      } else if (frame.kind === "retirement") {
        if (!owner.go || typeof frame.acknowledged !== "boolean" ||
            frame.ownerDirectory !== owner.directory) throw new Error("Invalid producer retirement receipt.");
        if (frame.acknowledged) {
          if (!Number.isInteger(frame.exitCode) && !frame.error) throw new Error("Producer retirement omitted execution outcome.");
          receipt = frame;
          owner.acknowledged = true;
          if (frame.error) failure ??= new Error("Producer failed: " + String(frame.error).slice(0, 4096));
          retireBytes();
        } else {
          reject(ownershipError("Windows producer retirement unacknowledged: " + String(frame.error ?? "").slice(0, 4096), owner));
        }
      } else throw new Error("Unknown Windows producer protocol frame.");
    };
    owner.child.once("error", (error) => stop(error));
    owner.child.once("close", (code, stoppedBy) => {
      owner.closed = true;
      signal?.removeEventListener("abort", abort);
      clearTimeout(timer);
      if (!owner.go) {
        owner.acknowledged = true;
        failure ??= new Error("PowerShell7 producer bootstrap failed (" + (code ?? stoppedBy) + "): " + stderr);
      } else if (!owner.acknowledged) {
        reject(ownershipError("Original producer host closed without an affirmative tree-retirement receipt.", owner, failure));
      } else if (code !== 0 && !failure && receipt?.exitCode === 0) {
        failure = new Error("Producer host failed after retirement (" + (code ?? stoppedBy) + "): " + stderr);
      }
      retireBytes();
    });
    owner.child.stdout.setEncoding("utf8");
    owner.child.stderr.setEncoding("utf8");
    owner.child.stdout.on("data", (chunk) => {
      try {
        text += chunk;
        while (text.includes("\n")) {
          const newline = text.indexOf("\n");
          if (newline > LINE_LIMIT) throw new Error("Producer protocol line exceeded its software envelope.");
          const line = text.slice(0, newline).replace(/\r$/, "");
          text = text.slice(newline + 1);
          if (line) handleFrame(JSON.parse(line));
        }
        if (text.length > LINE_LIMIT) throw new Error("Producer protocol line exceeded its software envelope.");
      } catch (error) { text = ""; stop(error); }
    });
    owner.child.stderr.on("data", (chunk) => {
      if (stderr.length < STDERR_LIMIT) stderr += chunk.slice(0, STDERR_LIMIT - stderr.length);
    });
    owner.child.stdin.on("error", (error) => stop(error));
    signal?.addEventListener("abort", abort, { once: true });
    timer = setTimeout(() => stop(new Error("PowerShell7 producer did not become ready before its startup deadline.")), STARTUP_MS);
    if (signal?.aborted) abort();
    else owner.child.stdin.write(requestText + "\n");
  } catch (error) {
    if (owner.child) stop(error);
    else {
      owner.closed = true;
      owner.acknowledged = true;
      failure = error;
      receipt = { exitCode: null };
      retireBytes();
    }
  }
  return result;
}
