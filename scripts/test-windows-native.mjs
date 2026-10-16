import { runOwnedWindowsCommand } from "./windows-owned-producer.mjs";
import { copyFile, lstat, mkdtemp, realpath, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// Only top-level test-helper activators are removed. Child fixtures deliberately
// supply their own activation values; production connection/lease policy is unchanged.
export const NATIVE_TEST_HELPER_ACTIVATORS = Object.freeze(["QC_HARNESS_DESCRIPTOR_PATH", "QC_HARNESS_STATUS_PATH", "QC_HARNESS_CONTROL_DIR", "QC_HARNESS_OBSERVER", "QC_DEVICE_LEASE_TEST_RACE_DIR", "QC_DEVICE_LEASE_TEST_RELEASE_PATH", "QC_REMOTE_LOCAL_MCP_STDERR_CHILD", "QC_REMOTE_TEST_STATE_ROOT"]);
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
  producerImpl = runOwnedWindowsCommand,
  transformCargoRequest = (request) => request,
  signalSource = process,
  stdout = process.stdout,
  stderr = process.stderr,
  platform = process.platform,
} = {}) {
  const root = resolve(rootDir);
  if (!existsSync(join(root, "Cargo.toml")) || !existsSync(join(root, APP_MANIFEST))) {
    throw new Error(`Windows native test root is missing Cargo manifests: ${root}`);
  }

  let interrupted = null;
  let retirementAcknowledged = true;
  const cancellation = new AbortController();
  let stagedBrokerDirectory = null;
  let stagedBrokerParent = null;
  let operationFailure = null;
  const onInterrupt = (signal) => {
    if (interrupted) return;
    interrupted = signal;
    cancellation.abort(new Error(`Interrupted by ${signal}.`));
  };
  const onSigint = () => onInterrupt("SIGINT");
  const onSigterm = () => onInterrupt("SIGTERM");
  signalSource.on("SIGINT", onSigint);
  signalSource.on("SIGTERM", onSigterm);

  const runCargo = async (args, { json = false, childEnv = env, label, nativeTestState = false }) => {
    if (interrupted) throw new Error("Interrupted by " + interrupted + ".");
    const messages = [];
    let stdoutBuffer = "";
    const emitLine = (line) => {
      if (!line) return;
      if (!json) { stdout.write(line + "\n"); return; }
      let message;
      try { message = JSON.parse(line); }
      catch { stdout.write(line + "\n"); return; }
      // Only artifacts select executables; diagnostics are streamed rather than
      // retaining an entire build's JSON output a second time.
      if (message.reason === "compiler-artifact") messages.push(message);
      if (message.reason === "compiler-message" && message.message?.rendered) {
        stderr.write(message.message.rendered);
      }
    };
    const request = transformCargoRequest({
      command: "rustup", arguments: cargoArguments(args), cwd: root,
      environment: { ...childEnv }, timeoutMilliseconds: 3_600_000,
      maxOutputBytes: 67_108_864,
      nativeTestState,
    });
    try {
      const result = await producerImpl(request, {
        signal: cancellation.signal,
        onStdout(chunk) {
          stdoutBuffer += chunk;
          const lines = stdoutBuffer.split(/\r?\n/);
          stdoutBuffer = lines.pop() ?? "";
          for (const line of lines) emitLine(line);
        },
        onStderr(chunk) { stderr.write(chunk); },
      });
      if (result.retirementAcknowledged !== true) {
        retirementAcknowledged = false;
        throw new Error(label + " returned without affirmative original-tree retirement.");
      }
      if (stdoutBuffer) emitLine(stdoutBuffer);
      if (interrupted) throw new Error("Interrupted by " + interrupted + ".");
      if (result.code !== 0) throw cargoError(label, result.code, null);
      return messages;
    } catch (error) {
      if (error.retirementAcknowledged === false) retirementAcknowledged = false;
      throw error;
    }
  };

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
    ], { childEnv: testEnv, label: "Windows app all-target tests", nativeTestState: true });
    return Object.freeze({ brokerExecutable: stagedBrokerExecutable, libraryTestExecutable, testMessages: testMessages.length });
  } catch (error) {
    operationFailure = error;
    throw error;
  } finally {
    signalSource.off("SIGINT", onSigint);
    signalSource.off("SIGTERM", onSigterm);
    if (stagedBrokerDirectory) {
      try {
        if (!retirementAcknowledged) {
          throw new Error("Retaining staged broker bytes because original Cargo tree retirement is unacknowledged: " + stagedBrokerDirectory);
        }
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
