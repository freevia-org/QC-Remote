import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(resolve(root, ".github/workflows/security-audit.yml"), "utf8");

// Tracked files only: ignored local output (target/, .artifacts/ probes) is not
// public and must not change the audit coverage this test expects.
function discoverFiles() {
  return execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 })
    .split("\0")
    .filter(Boolean);
}

test("public dependency audit names every public Cargo and Gradle lock and cloud workspace", () => {
  assert.match(workflow, /^on:\r?\n  workflow_dispatch:\r?\n/m, "security scans require explicit dispatch");
  assert.doesNotMatch(workflow, /^  (?:push|pull_request|pull_request_target|schedule):/m, "security scans must not run automatically");
  const files = discoverFiles();
  const cargoLocks = files.filter((path) => path === "Cargo.lock" || path.endsWith("/Cargo.lock"));
  const expectedRustDirectories = cargoLocks.map((path) => path === "Cargo.lock" ? "." : path.slice(0, -"/Cargo.lock".length)).sort();
  const matrix = /rustsec:[\s\S]*?directory:\s*\r?\n((?:\s*-\s+\S+\s*\r?\n)+)/.exec(workflow)?.[1];
  assert.ok(matrix, "RustSec matrix is missing, so no Cargo lock can be audited");
  const rustDirectories = [...matrix.matchAll(/^\s*-\s+(\S+)\s*$/gm)].map((match) => match[1]).sort();
  assert.deepEqual(rustDirectories, expectedRustDirectories, "RustSec coverage must match every public Cargo.lock exactly");

  const gradleLocks = files.filter((path) => path.endsWith("/gradle.lockfile"));
  assert.deepEqual(gradleLocks, ["apps/android/android/app/gradle.lockfile"], "expected exactly the Android app Gradle lock");
  assert.ok(workflow.includes("--lockfile=apps/android/android/app/gradle.lockfile"), "OSV scan omits the Android Gradle lock");
  assert.match(workflow, /npm ci --ignore-scripts/, "the manual audit must install the root locked dependencies");
  assert.match(workflow, /npm audit --workspace @qc-remote\/cloud --omit=dev --audit-level=moderate/);
  assert.match(workflow, /npm audit --workspace @qc-remote\/cloud --audit-level=high/);
  assert.match(workflow, /cache-dependency-path:\s*package-lock\.json/);
  assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
  assert.match(workflow, /osv-scanner-reusable\.yml@b1d267aea5211fa648d9d442fa64b90dbdcbdd36/);
  assert.match(workflow, /upload-sarif: false/, "the audit must not upload SARIF results");
  assert.match(workflow, /--lockfile=apps\/android\/android\/app\/gradle\.lockfile/);
  assert.match(workflow, /rustsec\/audit-check@[0-9a-f]{40}/);
  assert.doesNotMatch(workflow, /E:\\\\QC-Remote-GitHub|\.vendor\/qc-remote-public|services\/qc-remote-cloud\//,
    "the public workflow must run from its own repository, not a private junction/submodule path");

});
