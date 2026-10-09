import { spawn, execFile } from "node:child_process";
import { copyFile, lstat, mkdtemp, realpath, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// Only top-level test-helper activators are removed. Child fixtures deliberately
// supply their own activation values; production connection/lease policy is unchanged.
export const NATIVE_TEST_HELPER_ACTIVATORS = Object.freeze(["QC_HARNESS_DESCRIPTOR_PATH", "QC_HARNESS_STATUS_PATH", "QC_HARNESS_CONTROL_DIR", "QC_HARNESS_OBSERVER", "QC_DEVICE_LEASE_TEST_RACE_DIR", "QC_DEVICE_LEASE_TEST_RELEASE_PATH", "QC_REMOTE_LOCAL_MCP_STDERR_CHILD"]);
export function topLevelNativeTestEnvironment(environment) {
  const denied = new Set(NATIVE_TEST_HELPER_ACTIVATORS);
  return Object.fromEntries(Object.entries(environment).filter(([key]) => !denied.has(key.toUpperCase())));
}

export const WINDOWS_NATIVE_TOOLCHAIN = "stable-x86_64-pc-windows-msvc";
export const WINDOWS_NATIVE_TAURI_CONFIG = JSON.stringify({ bundle: { externalBin: [] } });

const SCRIPT_ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const APP_MANIFEST = "apps/windows/src-tauri/Cargo.toml";

function cargoArguments(args) {
  return ["run", WINDOWS_NATIVE_TOOLCHAIN, "cargo", ...args];
}

/** Parse one Cargo JSON stream and select the executable for the requested target. */
export function selectCargoExecutable(messages, { targetName, kind, test = false }) {
  const candidates = messages.filter((message) =>
    message?.reason === "compiler-artifact"
    && message.target?.name === targetName
    && Array.isArray(message.target.kind)
    && message.target.kind.includes(kind)
    && Array.isArray(message.target.crate_types)
    && message.target.crate_types.includes(kind)
    && Boolean(message.profile?.test) === test
    && typeof message.executable === "string"
    && message.executable.length > 0);
  if (candidates.length !== 1) {
    throw new Error(`Expected one Cargo executable for ${targetName} (${kind}, test=${test}); found ${candidates.length}.`);
  }
  return candidates[0].executable;
}

function parseRoot(argv, cwd) {
  if (argv.length === 0) return SCRIPT_ROOT;
  const index = argv.indexOf("--root");
  if (index !== 0 || argv.length !== 2) throw new Error("Only --root <repository-path> is supported.");
  const value = argv[index + 1];
  if (!value || value.startsWith("--")) throw new Error("--root requires a repository path.");
  return resolve(cwd, value);
}

export function waitForClose(child, milliseconds) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(true);
  return new Promise((settle) => {
    const finish = (closed) => {
      clearTimeout(timer);
      child.off("close", onClose);
      settle(closed);
    };
    const onClose = () => finish(true);
    const timer = setTimeout(() => finish(false), milliseconds);
    child.once("close", onClose);
  });
}

function taskkillTree(pid) {
  return new Promise((settle) => {
    execFile("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      windowsHide: true,
      timeout: 10_000,
    }, (error) => settle(error));
  });
}

/** Stop only the Cargo/rustup process tree launched by this runner. */
export async function terminateOwnedCargo(child, {
  platform = process.platform,
  graceMs = 2_000,
  killTree = taskkillTree,
} = {}) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  let treeError = null;
  if (platform === "win32" && Number.isInteger(child.pid) && child.pid > 0) {
    treeError = await killTree(child.pid);
  }
  if (await waitForClose(child, graceMs)) {
    if (treeError) {
      throw new Error(`Could not terminate the owned Windows Cargo process tree ${child.pid}: ${treeError.message}`, {
        cause: treeError,
      });
    }
    return;
  }
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  if (!await waitForClose(child, graceMs)) {
    throw new Error(`Cargo child ${child.pid ?? "without pid"} did not exit after termination.`);
  }
  if (treeError) {
    throw new Error(`Could not terminate the owned Windows Cargo process tree ${child.pid}: ${treeError.message}`, {
      cause: treeError,
    });
  }
}

function artifactPath(executable, rootDir) {
  const path = isAbsolute(executable) ? executable : resolve(rootDir, executable);
  if (!existsSync(path)) throw new Error(`Cargo reported a missing executable artifact: ${path}`);
  return path;
}

function cargoError(command, code, signal) {
  const detail = signal ? `signal ${signal}` : `exit code ${code}`;
  const error = new Error(`${command} failed with ${detail}.`);
  error.exitCode = Number.isInteger(code) ? code : 1;
  return error;
}

/**
 * Build exact MSVC artifacts and run the Windows app tests against those paths.
 * All toolchain and test-only environment changes are confined to child envs.
 */
export async function runWindowsNativeTests({
  rootDir = SCRIPT_ROOT,
  env = process.env,
  spawnImpl = spawn,
  terminateChild = terminateOwnedCargo,
  signalSource = process,
  stdout = process.stdout,
  stderr = process.stderr,
  platform = process.platform,
} = {}) {
  const root = resolve(rootDir);
  if (!existsSync(join(root, "Cargo.toml")) || !existsSync(join(root, APP_MANIFEST))) {
    throw new Error(`Windows native test root is missing Cargo manifests: ${root}`);
  }

  let child = null;
  let interrupted = null;
  let termination = null;
  let cleanupFailure = null;
  let stagedBrokerDirectory = null;
  let stagedBrokerParent = null;
  let operationFailure = null;
  const onInterrupt = (signal) => {
    if (interrupted) return;
    interrupted = signal;
    if (child) {
      termination = Promise.resolve(terminateChild(child, { platform }))
        .catch((error) => { cleanupFailure = error; });
    }
  };
  const onSigint = () => onInterrupt("SIGINT");
  const onSigterm = () => onInterrupt("SIGTERM");
  signalSource.on("SIGINT", onSigint);
  signalSource.on("SIGTERM", onSigterm);

  const runCargo = (args, { json = false, childEnv = env, label }) => new Promise((resolveRun, rejectRun) => {
    if (interrupted) return rejectRun(new Error(`Interrupted by ${interrupted}.`));
    const argv = cargoArguments(args);
    const command = "rustup";
    const runEnv = { ...childEnv };
    const processHandle = spawnImpl(command, argv, {
      cwd: root,
      env: runEnv,
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    });
    child = processHandle;
    const messages = [];
    let stdoutBuffer = "";
    let settled = false;
    const fail = (error) => {
      if (settled) return;
      settled = true;
      rejectRun(error);
    };
    const emitLine = (line) => {
      if (!line) return;
      if (!json) {
        stdout.write(`${line}\n`);
        return;
      }
      let message;
      try { message = JSON.parse(line); }
      catch {
        stdout.write(`${line}\n`);
        return;
      }
      messages.push(message);
      if (message.reason === "compiler-message" && message.message?.rendered) {
        stderr.write(message.message.rendered);
      }
    };
    const safeEmitLine = (line) => {
      try { emitLine(line); }
      catch (error) { fail(error); }
    };
    processHandle.stdout.setEncoding("utf8");
    processHandle.stderr.setEncoding("utf8");
    processHandle.stdout.on("data", (chunk) => {
      try {
        stdoutBuffer += chunk;
        const lines = stdoutBuffer.split(/\r?\n/);
        stdoutBuffer = lines.pop() ?? "";
        for (const line of lines) safeEmitLine(line);
      } catch (error) {
        fail(error);
      }
    });
    processHandle.stderr.on("data", (chunk) => {
      try { stderr.write(chunk); }
      catch (error) { fail(error); }
    });
    processHandle.once("error", (error) => fail(new Error(`${label} could not start: ${error.message}`, { cause: error })));
    processHandle.once("close", (code, signal) => {
      if (child === processHandle) child = null;
      if (stdoutBuffer) safeEmitLine(stdoutBuffer);
      if (settled) return;
      if (interrupted) return fail(new Error(`Interrupted by ${interrupted}.`));
      if (code !== 0) return fail(cargoError(label, code, signal));
      settled = true;
      resolveRun(messages);
    });
  });

  try {
    if (platform !== "win32") throw new Error("Windows native tests require a Windows host and the MSVC toolchain.");
    const tauriEnv = { ...topLevelNativeTestEnvironment(env), TAURI_CONFIG: WINDOWS_NATIVE_TAURI_CONFIG };
    const brokerMessages = await runCargo([
      "build", "--locked", "-p", "qc-device-broker", "--bin", "qc-device-broker", "--message-format=json",
    ], { json: true, childEnv: tauriEnv, label: "Windows broker build" });
    const brokerExecutable = artifactPath(selectCargoExecutable(brokerMessages, {
      targetName: "qc-device-broker", kind: "bin", test: false,
    }), root);
    // The app's tray-sidecar build emits the same target/debug executable name.
    // Keep the prepared workspace broker in an owned directory under that
    // target so later Cargo stages cannot replace the executable used by tests.
    stagedBrokerParent = await realpath(dirname(brokerExecutable));
    stagedBrokerDirectory = await mkdtemp(join(stagedBrokerParent, ".qc-device-broker-test-"));
    const stagedBrokerExecutable = join(stagedBrokerDirectory, basename(brokerExecutable));
    await copyFile(brokerExecutable, stagedBrokerExecutable);

    const harnessMessages = await runCargo([
      "test", "--locked", "--features", "tray-sidecar", "--lib", "--no-run", "--message-format=json",
      "--manifest-path", APP_MANIFEST,
    ], { json: true, childEnv: tauriEnv, label: "Windows app test harness build" });
    const libraryTestExecutable = artifactPath(selectCargoExecutable(harnessMessages, {
      targetName: "qc_remote_lib", kind: "rlib", test: true,
    }), root);

    const testEnv = {
      ...tauriEnv,
      QC_GATEWAY_EXECUTABLE: stagedBrokerExecutable,
      QC_REMOTE_LIB_TEST_EXECUTABLE: libraryTestExecutable,
    };
    const testMessages = await runCargo([
      "test", "--locked", "--features", "tray-sidecar", "--all-targets", "--manifest-path", APP_MANIFEST,
      "--", "--test-threads=1",
    ], { childEnv: testEnv, label: "Windows app all-target tests" });
    if (cleanupFailure) throw new Error(`Cargo child cleanup failed: ${cleanupFailure.message}`, { cause: cleanupFailure });
    return Object.freeze({ brokerExecutable: stagedBrokerExecutable, libraryTestExecutable, testMessages: testMessages.length });
  } catch (error) {
    if (child) {
      termination ??= Promise.resolve(terminateChild(child, { platform }))
        .catch((cleanupError) => { cleanupFailure = cleanupError; });
    }
    if (termination) await termination;
    operationFailure = cleanupFailure
      ? new AggregateError([error, cleanupFailure], `${error.message} Cleanup also failed: ${cleanupFailure.message}`)
      : error;
    throw operationFailure;
  } finally {
    signalSource.off("SIGINT", onSigint);
    signalSource.off("SIGTERM", onSigterm);
    if (termination) await termination;
    if (stagedBrokerDirectory) {
      try {
        const stageInfo = await lstat(stagedBrokerDirectory);
        if (stageInfo.isSymbolicLink() || !stageInfo.isDirectory() ||
            dirname(stagedBrokerDirectory) !== stagedBrokerParent ||
            !basename(stagedBrokerDirectory).startsWith(".qc-device-broker-test-")) {
          throw new Error("Refusing to remove a broker staging directory outside its owned target parent.");
        }
        // Remove the original entry, never a resolved replacement link's target.
        await rm(stagedBrokerDirectory, { recursive: true, force: true });
      } catch (cleanupError) {
        if (operationFailure) {
          throw new AggregateError([operationFailure, cleanupError], `${operationFailure.message} Staged broker cleanup also failed: ${cleanupError.message}`);
        }
        throw cleanupError;
      }
    }
  }
}

function mainRoot(argv) {
  return parseRoot(argv, process.cwd());
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const rootDir = mainRoot(process.argv.slice(2));
    const result = await runWindowsNativeTests({ rootDir });
    process.stdout.write(`Windows app tests passed.\nTemporary broker used (cleaned): ${result.brokerExecutable}\nLibrary test harness: ${result.libraryTestExecutable}\n`);
  } catch (error) {
    process.stderr.write(`${error.stack ?? error}\n`);
    process.exitCode = Number.isInteger(error.exitCode) ? error.exitCode : 1;
  }
}
