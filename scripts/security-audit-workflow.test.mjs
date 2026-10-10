import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = readFileSync(resolve(root, ".github/workflows/security-audit.yml"), "utf8");

function discoverFiles(directory, found = []) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (entry.name === ".git" || entry.name === "node_modules" || entry.name === "target" || entry.name === "build") continue;
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) discoverFiles(path, found);
    else found.push(relative(root, path).replaceAll("\\", "/"));
  }
  return found;
}

test("public dependency audit names every public Cargo and Gradle lock and cloud workspace", () => {
  const files = discoverFiles(root);
  const cargoLocks = files.filter((path) => path === "Cargo.lock" || path.endsWith("/Cargo.lock"));
  const expectedRustDirectories = cargoLocks.map((path) => path === "Cargo.lock" ? "." : path.slice(0, -"/Cargo.lock".length)).sort();
  const matrix = /rustsec:[\s\S]*?directory:\s*\r?\n((?:\s*-\s+\S+\s*\r?\n)+)/.exec(workflow)?.[1];
  assert.ok(matrix, "RustSec matrix is missing, so no Cargo lock can be audited");
  const rustDirectories = [...matrix.matchAll(/^\s*-\s+(\S+)\s*$/gm)].map((match) => match[1]).sort();
  assert.deepEqual(rustDirectories, expectedRustDirectories, "RustSec coverage must match every public Cargo.lock exactly");

  const gradleLocks = files.filter((path) => path.endsWith("/gradle.lockfile"));
  assert.deepEqual(gradleLocks, ["apps/android/android/app/gradle.lockfile"], "expected exactly the Android app Gradle lock");
  assert.ok(workflow.includes("--lockfile=apps/android/android/app/gradle.lockfile"), "OSV scan omits the Android Gradle lock");
  assert.match(workflow, /- "\*\*\/Cargo\.lock"/, "Cargo lock changes must trigger the audit");
  assert.match(workflow, /- "\*\*\/gradle\.lockfile"/, "Gradle lock changes must trigger the audit");
  assert.match(workflow, /- "\*\*\/\*\.gradle"/, "Gradle build-file changes must trigger the audit");
  assert.match(workflow, /- "\*\*\/\*\.gradle\.kts"/, "Gradle Kotlin build-file changes must trigger the audit");
  assert.match(workflow, /- "package-lock\.json"/, "npm lock changes must trigger the audit");
  assert.match(workflow, /npm audit --workspace @qc-remote\/cloud --omit=dev --audit-level=moderate/);
  assert.match(workflow, /npm audit --workspace @qc-remote\/cloud --audit-level=high/);
  assert.match(workflow, /cache-dependency-path:\s*package-lock\.json/);
  assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
  assert.match(workflow, /osv-scanner-reusable\.yml@a345acffa64b0eaede81a3d9aae6141214d9c8fc/);
  assert.match(workflow, /upload-sarif: false/, "fork PR scans must not require a write-scoped SARIF upload");
  assert.match(workflow, /--lockfile=apps\/android\/android\/app\/gradle\.lockfile/);
  assert.match(workflow, /rustsec\/audit-check@[0-9a-f]{40}/);
  assert.doesNotMatch(workflow, /E:\\\\QC-Remote-GitHub|\.vendor\/qc-remote-public|services\/qc-remote-cloud\//,
    "the public workflow must run from its own repository, not a private junction/submodule path");

});
