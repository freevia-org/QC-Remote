import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { readFileSync, writeFileSync, renameSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { PassThrough } from "node:stream";
import test from "node:test";
import {
  runWindowsNativeTests as runNative,
  NATIVE_TEST_HELPER_ACTIVATORS,
  topLevelNativeTestEnvironment,
  selectCargoExecutable,
} from "./test-windows-native.mjs";

function artifact(targetName, kind, executable, testProfile = false) {
  return {
    reason: "compiler-artifact",
    target: { name: targetName, kind: [kind], crate_types: [kind] },
    profile: { test: testProfile },
    executable,
  };
}

function mockChild({ messages = [], exitCode = 0, hold = false } = {}) {
  const child = new EventEmitter();
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.exitCode = null;
  child.signalCode = null;
  child.killCalls = [];
  child.kill = (signal) => {
    child.killCalls.push(signal);
    child.close(130, signal);
    return true;
  };
  child.close = (code, signal = null) => {
    if (child.exitCode !== null || child.signalCode !== null) return;
    child.exitCode = signal ? null : code;
    child.signalCode = signal;
    child.stdout.end();
    child.stderr.end();
    child.emit("close", code, signal);
  };
  if (!hold) setImmediate(() => {
    for (const message of messages) child.stdout.write(`${JSON.stringify(message)}\n`);
    child.close(exitCode);
  });
  return child;
}


// Explicit producer test double, never the production default or native proof.
// Existing artifact/environment/staging oracles below are unchanged.
function mockOwnedProducer(spawnImpl, terminateChild = async (child) => child.close(130, "SIGINT")) {
  return (request, { signal, onStdout, onStderr }) => new Promise((resolve, reject) => {
    const child = spawnImpl(request.command, request.arguments, {
      cwd: request.cwd, env: request.environment,
      nativeTestState: request.nativeTestState,
      stdio: ["ignore", "pipe", "pipe"], windowsHide: true,
    });
    let completed = false;
    let retiring = null;
    let stopping = false;
    const fail = (error) => {
      if (completed || stopping) return;
      stopping = true;
      retiring = (async () => {
        try {
          await terminateChild(child, { platform: "win32" });
          error.retirementAcknowledged = true;
        } catch (cleanup) {
          error = new AggregateError([error, cleanup], error.message + " Cleanup: " + cleanup.message);
          error.retirementAcknowledged = false;
        }
        completed = true;
        signal?.removeEventListener("abort", abort);
        reject(error);
      })();
    };
    const abort = () => fail(new Error("Interrupted by SIGINT."));
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { try { onStdout(chunk); } catch (error) { fail(error); } });
    child.stderr.on("data", (chunk) => { try { onStderr(chunk); } catch (error) { fail(error); } });
    child.once("error", fail);
    child.once("close", (code) => {
      if (completed || stopping) return;
      completed = true;
      signal?.removeEventListener("abort", abort);
      resolve({ code, retirementAcknowledged: true });
    });
    signal?.addEventListener("abort", abort, { once: true });
    if (signal?.aborted) abort();
  });
}

function runWindowsNativeTests({ spawnImpl, terminateChild, ...options }) {
  return runNative({ ...options, producerImpl: mockOwnedProducer(spawnImpl, terminateChild) });
}

const quiet = { write() {} };
const rootDir = process.cwd();

async function temporaryRepository() {
  const root = await mkdtemp(join(tmpdir(), "qc-windows-native-test-"));
  await mkdir(join(root, "apps", "windows", "src-tauri"), { recursive: true });
  await writeFile(join(root, "Cargo.toml"), "[workspace]\n");
  await writeFile(join(root, "apps", "windows", "src-tauri", "Cargo.toml"), "[package]\n");
  const targetDir = join(root, "target", "debug");
  await mkdir(targetDir, { recursive: true });
  const brokerPath = join(targetDir, "qc-device-broker.exe");
  await writeFile(brokerPath, "prepared workspace broker");
  return { root, targetDir, brokerPath };
}

test("compiler artifact selection requires the exact target kind and test profile", () => {
  const selected = selectCargoExecutable([
    artifact("qc-device-broker", "bin", "C:\\target\\debug\\qc-device-broker.exe"),
    artifact("qc_remote_lib", "rlib", "C:\\target\\debug\\deps\\qc_remote_lib-test.exe", true),
    artifact("qc_remote_lib", "rlib", "C:\\target\\debug\\deps\\qc_remote_lib.rlib"),
  ], { targetName: "qc_remote_lib", kind: "rlib", test: true });
  assert.equal(selected, "C:\\target\\debug\\deps\\qc_remote_lib-test.exe");
  assert.throws(() => selectCargoExecutable([], {
    targetName: "qc_remote_lib", kind: "rlib", test: true,
  }), /found 0/);
  assert.throws(() => selectCargoExecutable([
    artifact("qc_remote_lib", "rlib", "one.exe", true),
    artifact("qc_remote_lib", "rlib", "two.exe", true),
  ], { targetName: "qc_remote_lib", kind: "rlib", test: true }), /found 2/);
});

test("the library harness artifact matches Cargo's rlib target shape", () => {
  const event = {
    reason: "compiler-artifact",
    target: {
      name: "qc_remote_lib",
      kind: ["rlib"],
      crate_types: ["rlib"],
    },
    profile: { test: true },
    executable: "C:\\target\\debug\\deps\\qc_remote_lib-123.exe",
  };
  assert.equal(selectCargoExecutable([event], {
    targetName: "qc_remote_lib", kind: "rlib", test: true,
  }), event.executable);
  assert.throws(() => selectCargoExecutable([{
    ...event, target: { ...event.target, kind: ["lib"], crate_types: ["rlib"] },
  }], { targetName: "qc_remote_lib", kind: "rlib", test: true }), /found 0/);
});

test("preserves the prepared broker across an app build collision and scopes child variables", async () => {
  const fixture = await temporaryRepository();
  const calls = [];
  const env = {
    TAURI_CONFIG: "caller-tauri-config",
    QC_GATEWAY_EXECUTABLE: "caller-gateway",
    QC_REMOTE_LIB_TEST_EXECUTABLE: "caller-harness",
    CARGO_TARGET_DIR: "E:\\cache\\cargo-target",
    PRESERVE_ME: "yes",
    ...Object.fromEntries(NATIVE_TEST_HELPER_ACTIVATORS.map((key) => [key, "foreign-fixture"])),
    qc_harness_descriptor_path: "lowercase-foreign",
    qC_rEmOtE_tEsT_sTaTe_RoOt: "foreign-unowned-state",
  };
  const original = { ...env };
  const spawnImpl = (command, args, options) => {
    calls.push({ command, args, options });
    if (args.includes("--bin")) return mockChild({
      messages: [artifact("other-bin", "bin", process.execPath), artifact("qc-device-broker", "bin", fixture.brokerPath)],
    });
    if (args.includes("--no-run")) return mockChild({
      messages: [artifact("qc_remote_lib", "rlib", process.execPath, true)],
    });
    if (args.includes("--all-targets")) {
      writeFileSync(fixture.brokerPath, "app tray-sidecar broker overwrote target");
      const stagedBroker = options.env.QC_GATEWAY_EXECUTABLE;
      assert.equal(readFileSync(stagedBroker, "utf8"), "prepared workspace broker");
      assert.equal(readFileSync(fixture.brokerPath, "utf8"), "app tray-sidecar broker overwrote target");
      return mockChild({ exitCode: 0 });
    }
    return mockChild({ exitCode: 0 });
  };

  try {
    const result = await runWindowsNativeTests({
      rootDir: fixture.root, env, spawnImpl, platform: "win32", signalSource: new EventEmitter(), stdout: quiet, stderr: quiet,
    });

    assert.deepEqual(env, original, "the parent environment is not mutated");
    assert.equal(calls.length, 3);
    assert.equal(NATIVE_TEST_HELPER_ACTIVATORS.length, 8);
    assert.deepEqual(calls.map((call) => call.options.nativeTestState), [false, false, true],
      "only the executing test producer owns native health/stderr state");
    for (const call of calls) {
      assert.equal(call.options.env.PRESERVE_ME, "yes");
      for (const key of NATIVE_TEST_HELPER_ACTIVATORS) assert.equal(call.options.env[key], undefined);
      assert.equal(call.options.env.qc_harness_descriptor_path, undefined);
      assert.equal(call.options.env.qC_rEmOtE_tEsT_sTaTe_RoOt, undefined);
    }
    assert.ok(calls.every((call) => call.command === "rustup"));
    assert.ok(calls.every((call) => call.args.slice(0, 3).join(" ") === "run stable-x86_64-pc-windows-msvc cargo"));
    assert.ok(calls.every((call) => call.options.cwd === fixture.root));
    assert.equal(calls[0].args.includes("--locked"), true);
    assert.equal(calls[0].options.env.CARGO_TARGET_DIR, env.CARGO_TARGET_DIR);
    assert.equal(calls[0].options.env.TAURI_CONFIG, '{"bundle":{"externalBin":[]}}');
    assert.deepEqual(calls[1].args.slice(3, 10), ["test", "--locked", "--features", "tray-sidecar", "--lib", "--no-run", "--message-format=json"]);
    assert.equal(calls[1].options.env.TAURI_CONFIG, '{"bundle":{"externalBin":[]}}');
    assert.equal(calls[1].options.env.QC_GATEWAY_EXECUTABLE, env.QC_GATEWAY_EXECUTABLE);
    assert.deepEqual(calls[2].args.slice(3, 8), ["test", "--locked", "--features", "tray-sidecar", "--all-targets"]);
    assert.equal(calls[2].options.env.TAURI_CONFIG, '{"bundle":{"externalBin":[]}}');
    assert.equal(calls[2].args[8], "--manifest-path");
    assert.equal(calls[2].args[9], "apps/windows/src-tauri/Cargo.toml");
    assert.deepEqual(calls[2].args.slice(10), ["--", "--test-threads=1"]);
    assert.notEqual(calls[2].options.env.QC_GATEWAY_EXECUTABLE, fixture.brokerPath);
    assert.equal(calls[2].options.env.QC_REMOTE_LIB_TEST_EXECUTABLE, process.execPath);
    assert.equal(result.brokerExecutable, calls[2].options.env.QC_GATEWAY_EXECUTABLE);
    assert.equal(result.libraryTestExecutable, process.execPath);
    await assert.rejects(readFile(result.brokerExecutable), { code: "ENOENT" }, "the owned staging directory is removed after tests");
    assert.deepEqual(await readdir(fixture.targetDir), ["qc-device-broker.exe"]);
  } finally {
    await rm(fixture.root, { recursive: true, force: true });
  }
});

test("a failed harness compilation stops before tests and rejects missing compiler artifacts", async () => {
  const fixture = await temporaryRepository();
  const calls = [];
  const spawnImpl = (command, args, options) => {
    calls.push({ args, options });
    return mockChild({ messages: args.includes("--bin")
      ? [artifact("qc-device-broker", "bin", fixture.brokerPath)]
      : [artifact("qc_remote_lib", "rlib", null, true)] });
  };
  try {
    await assert.rejects(runWindowsNativeTests({
      rootDir: fixture.root, spawnImpl, platform: "win32", signalSource: new EventEmitter(), stdout: quiet, stderr: quiet,
    }), /Expected one Cargo executable for qc_remote_lib/);
    assert.equal(calls.length, 2, "the app test command must not run without an exact harness artifact");
    assert.deepEqual(await readdir(fixture.targetDir), ["qc-device-broker.exe"], "staged broker is removed after build failure");
  } finally {
    await rm(fixture.root, { recursive: true, force: true });
  }
});

test("interruption terminates the owned child tree and prevents later Cargo stages", async () => {
  const calls = [];
  const signalSource = new EventEmitter();
  let childToStop;
  const spawnImpl = (command, args, options) => {
    calls.push({ args, options });
    childToStop = mockChild({ hold: true });
    setImmediate(() => signalSource.emit("SIGINT"));
    return childToStop;
  };
  const stopped = [];
  await assert.rejects(runWindowsNativeTests({
    rootDir,
    spawnImpl,
    platform: "win32",
    signalSource,
    stdout: quiet,
    stderr: quiet,
    terminateChild: async (child) => { stopped.push(child); child.close(130, "SIGINT"); },
  }), /Interrupted by SIGINT/);
  assert.deepEqual(stopped, [childToStop]);
  assert.equal(calls.length, 1, "no later build or test starts after interruption");
});

test("an interruption after staging removes the temporary broker", async () => {
  const fixture = await temporaryRepository();
  const signalSource = new EventEmitter();
  let calls = 0;
  try {
    await assert.rejects(runWindowsNativeTests({
      rootDir: fixture.root, platform: "win32", signalSource, stdout: quiet, stderr: quiet,
      spawnImpl: () => {
        calls += 1;
        if (calls === 1) return mockChild({ messages: [artifact("qc-device-broker", "bin", fixture.brokerPath)] });
        const child = mockChild({ hold: true });
        setImmediate(() => signalSource.emit("SIGINT"));
        return child;
      },
      terminateChild: async (child) => child.close(130, "SIGINT"),
    }), /Interrupted by SIGINT/);
    assert.equal(calls, 2);
    assert.deepEqual(await readdir(fixture.targetDir), ["qc-device-broker.exe"]);
  } finally {
    await rm(fixture.root, { recursive: true, force: true });
  }
});

test("cleanup refuses a staging path replaced with a sibling junction", async () => {
  const fixture = await temporaryRepository();
  const sibling = join(fixture.targetDir, ".qc-device-broker-test-preserved");
  try {
    await assert.rejects(runWindowsNativeTests({
      rootDir: fixture.root, platform: "win32", signalSource: new EventEmitter(), stdout: quiet, stderr: quiet,
      spawnImpl: (_command, args, options) => {
        if (args.includes("--bin")) return mockChild({ messages: [artifact("qc-device-broker", "bin", fixture.brokerPath)] });
        if (args.includes("--no-run")) return mockChild({ messages: [artifact("qc_remote_lib", "rlib", process.execPath, true)] });
        const staging = dirname(options.env.QC_GATEWAY_EXECUTABLE);
        renameSync(staging, sibling);
        symlinkSync(sibling, staging, "junction");
        return mockChild();
      },
    }), /Refusing to remove a broker staging directory/);
    assert.equal(await readFile(join(sibling, "qc-device-broker.exe"), "utf8"), "prepared workspace broker");
  } finally {
    await rm(fixture.root, { recursive: true, force: true });
  }
});

test("a Cargo spawn error cleans up its owned child before returning", async () => {
  const signalSource = new EventEmitter();
  const child = mockChild({ hold: true });
  const spawnImpl = () => {
    setImmediate(() => child.emit("error", new Error("rustup not found")));
    return child;
  };
  let cleaned = false;
  await assert.rejects(runWindowsNativeTests({
    rootDir,
    spawnImpl,
    platform: "win32",
    signalSource,
    stdout: quiet,
    stderr: quiet,
    terminateChild: async (owned) => { cleaned = owned === child; child.close(1); },
  }), /rustup not found/);
  assert.equal(cleaned, true);
});

test("a child output sink failure is contained and triggers owned child cleanup", async () => {
  const child = mockChild({ hold: true });
  const spawnImpl = () => {
    setImmediate(() => child.stdout.write(`${JSON.stringify({
      reason: "compiler-message", message: { rendered: "diagnostic" },
    })}\n`));
    return child;
  };
  let cleaned = false;
  await assert.rejects(runWindowsNativeTests({
    rootDir,
    spawnImpl,
    platform: "win32",
    signalSource: new EventEmitter(),
    stdout: quiet,
    stderr: { write() { throw new Error("output sink closed"); } },
    terminateChild: async (owned) => { cleaned = owned === child; child.close(130); },
  }), /output sink closed/);
  assert.equal(cleaned, true);
});

test("a nonzero all-target test result is propagated without losing its exit code", async () => {
  const fixture = await temporaryRepository();
  const calls = [];
  const spawnImpl = (command, args, options) => {
    calls.push({ args, options });
    if (args.includes("--bin")) return mockChild({ messages: [artifact("qc-device-broker", "bin", fixture.brokerPath)] });
    if (args.includes("--no-run")) return mockChild({ messages: [artifact("qc_remote_lib", "rlib", process.execPath, true)] });
    return mockChild({ exitCode: 17 });
  };
  try {
    await assert.rejects(runWindowsNativeTests({
      rootDir: fixture.root, spawnImpl, platform: "win32", signalSource: new EventEmitter(), stdout: quiet, stderr: quiet,
    }), (error) => error.exitCode === 17 && /all-target tests failed/.test(error.message));
    assert.equal(calls.length, 3);
    assert.deepEqual(await readdir(fixture.targetDir), ["qc-device-broker.exe"], "staged broker is removed after test failure");
  } finally {
    await rm(fixture.root, { recursive: true, force: true });
  }
});

test("explicit child helper activation survives outside top-level sanitization", () => {
  const original = { QC_HARNESS_DESCRIPTOR_PATH: "foreign", QC_DEVICE_LEASE_PATH: "production-lease", PRESERVE_ME: "yes" };
  const top = topLevelNativeTestEnvironment(original);
  assert.equal(top.QC_HARNESS_DESCRIPTOR_PATH, undefined);
  assert.equal(top.QC_DEVICE_LEASE_PATH, "production-lease");
  const child = { ...top, QC_HARNESS_DESCRIPTOR_PATH: "owned", QC_HARNESS_CONTROL_DIR: "owned-control" };
  assert.equal(child.QC_HARNESS_DESCRIPTOR_PATH, "owned");
  assert.equal(child.QC_HARNESS_CONTROL_DIR, "owned-control");
  assert.equal(original.QC_HARNESS_DESCRIPTOR_PATH, "foreign");
});



test("unknown original retirement retains staged broker bytes instead of reporting cleanup", async () => {
  const fixture = await temporaryRepository();
  let count = 0;
  let staged;
  try {
    await assert.rejects(runNative({
      rootDir: fixture.root, platform: "win32", signalSource: new EventEmitter(), stdout: quiet, stderr: quiet,
      producerImpl: async (request, { onStdout }) => {
        if (++count === 1) {
          onStdout(JSON.stringify(artifact("qc-device-broker", "bin", fixture.brokerPath)) + "\n");
          return { code: 0, retirementAcknowledged: true };
        }
        staged = (await readdir(fixture.targetDir)).find((name) => name.startsWith(".qc-device-broker-test-"));
        const error = new Error("injected unknown original retirement");
        error.retirementAcknowledged = false;
        throw error;
      },
    }), /Retaining staged broker bytes/);
    assert.equal(count, 2, "no subsequent Cargo dispatch after unknown retirement");
    assert.equal(await readFile(join(fixture.targetDir, staged, "qc-device-broker.exe"), "utf8"), "prepared workspace broker");
  } finally {
    // This test double launched no native process; exact owned fixture bytes
    // are safe to retire independently after checking production retention.
    await rm(fixture.root, { recursive: true, force: true });
  }
});
