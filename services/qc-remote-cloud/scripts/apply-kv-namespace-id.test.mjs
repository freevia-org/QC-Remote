// Pins the split between the dry-run path (may use a labelled placeholder KV
// id) and the deploy path (must never). Run with `npm test`.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { unstable_readConfig } from "wrangler";

import {
  DEPLOY_CONFIG,
  DRY_RUN_CONFIG,
  DRY_RUN_PLACEHOLDER_ID,
  ENV_VAR,
  TEMPLATE_TOKEN,
  deployabilityProblem,
  main,
  resolveNamespaceId,
} from "./apply-kv-namespace-id.mjs";

const SERVICE_DIR = dirname(dirname(fileURLToPath(import.meta.url)));
const REAL_ID = "0123456789abcdef0123456789abcdef";

function withServiceCopy(run) {
  const dir = mkdtempSync(join(tmpdir(), "qcrc-kv-"));
  try {
    copyFileSync(join(SERVICE_DIR, "wrangler.jsonc"), join(dir, "wrangler.jsonc"));
    return run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function runMain(argv, { env = {}, serviceDir }) {
  const out = [];
  const err = [];
  const status = main(argv, { env, serviceDir, log: (line) => out.push(line), error: (line) => err.push(line) });
  return { status, out: out.join("\n"), err: err.join("\n") };
}

test("dry run without the secret binds the labelled placeholder in its own file", () => {
  withServiceCopy((dir) => {
    const result = runMain(["--dry-run"], { serviceDir: dir });
    assert.equal(result.status, 0, result.err);
    assert.match(result.out, /DRY-RUN PLACEHOLDER/);
    const configPath = join(dir, DRY_RUN_CONFIG);
    const text = readFileSync(configPath, "utf8");
    assert.ok(text.includes(`"id": "${DRY_RUN_PLACEHOLDER_ID}"`));
    assertWorkersLogRedaction(configPath, DRY_RUN_CONFIG);
    assert.ok(!text.includes(TEMPLATE_TOKEN));
    assert.equal(existsSync(join(dir, DEPLOY_CONFIG)), false, "a dry run must not write the deployable config");
  });
});

test("dry run with the secret uses the real id", () => {
  withServiceCopy((dir) => {
    const result = runMain(["--dry-run"], { serviceDir: dir, env: { [ENV_VAR]: REAL_ID } });
    assert.equal(result.status, 0, result.err);
    assert.doesNotMatch(result.out, /PLACEHOLDER/);
    const configPath = join(dir, DRY_RUN_CONFIG);
    const text = readFileSync(configPath, "utf8");
    assert.ok(text.includes(REAL_ID));
    assertWorkersLogRedaction(configPath, DRY_RUN_CONFIG);
  });
});

test("deploy config generation refuses a missing secret", () => {
  withServiceCopy((dir) => {
    const result = runMain([], { serviceDir: dir });
    assert.equal(result.status, 1);
    assert.match(result.err, /Missing QCRC_OAUTH_KV_NAMESPACE_ID/);
    assert.equal(existsSync(join(dir, DEPLOY_CONFIG)), false);
  });
});

test("config generation refuses unsafe observability values and later duplicate overrides", () => {
  withServiceCopy((dir) => {
    const templatePath = join(dir, "wrangler.jsonc");
    const original = readFileSync(templatePath, "utf8");
    const cases = [
      original.replace('"redact_query_string": true', '"redact_query_string": false'),
      original.replace('"redact_query_string": true,\n\t\t"head_sampling_rate": 0.01', '"redact_query_string": true,\n\t\t"head_sampling_rate": 0.01,\n\t\t"redact_query_string": false'),
      original.replace('"head_sampling_rate": 0.01', '"head_sampling_rate": 0.5'),
      original.replace('"head_sampling_rate": 0.01', '"head_sampling_rate": 0.01,\n\t\t"head_sampling_rate": 0.5'),
    ];
    for (const [index, unsafeTemplate] of cases.entries()) {
      writeFileSync(templatePath, unsafeTemplate);
      const deploy = runMain([], { serviceDir: dir, env: { [ENV_VAR]: REAL_ID } });
      assert.equal(deploy.status, 1, `deploy config case ${index + 1} accepted`);
      assert.match(deploy.err, /query-string redaction and 1% observability sampling/);
      assert.equal(existsSync(join(dir, DEPLOY_CONFIG)), false);

      const dryRun = runMain(["--dry-run"], { serviceDir: dir });
      assert.equal(dryRun.status, 1, `dry-run config case ${index + 1} accepted`);
      assert.match(dryRun.err, /query-string redaction and 1% observability sampling/);
      assert.equal(existsSync(join(dir, DRY_RUN_CONFIG)), false);
    }
  });
});

test("deploy config generation refuses the placeholder even when supplied explicitly", () => {
  withServiceCopy((dir) => {
    for (const value of [DRY_RUN_PLACEHOLDER_ID, DRY_RUN_PLACEHOLDER_ID.toUpperCase(), ` ${DRY_RUN_PLACEHOLDER_ID} `]) {
      const result = runMain([], { serviceDir: dir, env: { [ENV_VAR]: value } });
      assert.equal(result.status, 1, `accepted ${JSON.stringify(value)}`);
      assert.match(result.err, /dry-run placeholder/);
      assert.equal(existsSync(join(dir, DEPLOY_CONFIG)), false);
    }
  });
});

test("a malformed id is refused in both modes", () => {
  for (const dryRun of [false, true]) {
    assert.ok(resolveNamespaceId({ env: { [ENV_VAR]: "not-an-id" }, dryRun }).error);
  }
});

test("deploy config generation accepts a real id", () => {
  withServiceCopy((dir) => {
    const result = runMain([], { serviceDir: dir, env: { [ENV_VAR]: REAL_ID } });
    assert.equal(result.status, 0, result.err);
    const configPath = join(dir, DEPLOY_CONFIG);
    const text = readFileSync(configPath, "utf8");
    assert.ok(text.includes(REAL_ID));
    assertWorkersLogRedaction(configPath, DEPLOY_CONFIG);
    assert.equal(runMain(["--verify-deployable"], { serviceDir: dir }).status, 0);
  });
});

function assertWorkersLogRedaction(configPath, configName) {
  const config = unstable_readConfig({ config: configPath });
  assert.equal(config.observability?.redact_query_string, true, `${configName} must redact query strings`);
  assert.equal(config.observability?.head_sampling_rate, 0.01, `${configName} must retain 1% log sampling`);
}

test("the canonical Wrangler config redacts request query strings at 1% sampling", () => {
  assertWorkersLogRedaction(join(SERVICE_DIR, "wrangler.jsonc"), "wrangler.jsonc");
});

test("the deploy guard rejects false and later duplicate observability settings", () => {
  withServiceCopy((dir) => {
    const generated = runMain([], { serviceDir: dir, env: { [ENV_VAR]: REAL_ID } });
    assert.equal(generated.status, 0, generated.err);
    const deployPath = join(dir, DEPLOY_CONFIG);
    const validConfig = readFileSync(deployPath, "utf8");
    const unsafeConfigs = [
      validConfig.replace('"redact_query_string": true', '"redact_query_string": false'),
      validConfig.replace('"redact_query_string": true,\n\t\t"head_sampling_rate": 0.01', '"redact_query_string": true,\n\t\t"head_sampling_rate": 0.01,\n\t\t"redact_query_string": false'),
      validConfig.replace('"head_sampling_rate": 0.01', '"head_sampling_rate": 0.5'),
      validConfig.replace('"head_sampling_rate": 0.01', '"head_sampling_rate": 0.01,\n\t\t"head_sampling_rate": 0.5'),
    ];

    for (const [index, unsafeConfig] of unsafeConfigs.entries()) {
      writeFileSync(deployPath, unsafeConfig);
      const result = runMain(["--verify-deployable"], { serviceDir: dir });
      assert.equal(result.status, 1, `unsafe deploy config ${index + 1} accepted`);
      assert.match(result.err, /query-string redaction and 1% observability sampling are required/);
    }
  });
});

test("the deploy guard rejects a deployable config carrying the placeholder, the template, or nothing", () => {
  withServiceCopy((dir) => {
    const template = readFileSync(join(dir, "wrangler.jsonc"), "utf8");
    const deployPath = join(dir, DEPLOY_CONFIG);

    assert.equal(runMain(["--verify-deployable"], { serviceDir: dir }).status, 1, "missing file accepted");

    // e.g. someone copied wrangler.check.jsonc over it by hand
    writeFileSync(deployPath, template.split(TEMPLATE_TOKEN).join(DRY_RUN_PLACEHOLDER_ID));
    const placeholder = runMain(["--verify-deployable"], { serviceDir: dir });
    assert.equal(placeholder.status, 1);
    assert.match(placeholder.err, /placeholder/);

    writeFileSync(deployPath, template);
    assert.equal(runMain(["--verify-deployable"], { serviceDir: dir }).status, 1, "template accepted");

    // one real binding and one placeholder binding is still not deployable
    writeFileSync(deployPath, template.replace(TEMPLATE_TOKEN, REAL_ID).replace(TEMPLATE_TOKEN, DRY_RUN_PLACEHOLDER_ID));
    assert.equal(runMain(["--verify-deployable"], { serviceDir: dir }).status, 1, "mixed config accepted");
  });
  assert.equal(deployabilityProblem('{"id": "' + REAL_ID + '"}'), null);
});

test("the CLI runs when invoked through a directory junction or symlink", () => {
  // Other tooling may invoke this script through a directory junction or
  // symlink; argv[1] then names the link while import.meta.url names the target.
  withServiceCopy((dir) => {
    mkdirSync(join(dir, "scripts"));
    copyFileSync(fileURLToPath(new URL("./apply-kv-namespace-id.mjs", import.meta.url)), join(dir, "scripts", "apply-kv-namespace-id.mjs"));
    const link = `${dir}-link`;
    symlinkSync(dir, link, "junction");
    try {
      const env = { ...process.env };
      delete env[ENV_VAR];
      const result = spawnSync(process.execPath, [join(link, "scripts", "apply-kv-namespace-id.mjs"), "--dry-run"], { env, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /Wrote /);
      assert.ok(readFileSync(join(dir, DRY_RUN_CONFIG), "utf8").includes(DRY_RUN_PLACEHOLDER_ID));
      const refused = spawnSync(process.execPath, [join(link, "scripts", "apply-kv-namespace-id.mjs")], { env, encoding: "utf8" });
      assert.equal(refused.status, 1, "the deploy-config path must exit non-zero, not silently skip");
    } finally {
      rmSync(link, { recursive: false, force: true });
    }
  });
});

test("package scripts keep the placeholder on the dry-run path only", () => {
  const { scripts } = JSON.parse(readFileSync(join(SERVICE_DIR, "package.json"), "utf8"));
  assert.equal(scripts["kv:configure:dry-run"], "node scripts/apply-kv-namespace-id.mjs --dry-run");
  for (const name of ["check", "deploy:dry"]) {
    assert.ok(scripts[name].startsWith("npm run kv:configure:dry-run &&"), `${name} should generate the dry-run config`);
    assert.ok(!scripts[name].includes(DEPLOY_CONFIG), `${name} should not read ${DEPLOY_CONFIG}`);
    assert.ok(
      [...scripts[name].matchAll(/wrangler deploy\b[^&]*/g)].every((match) => match[0].includes("--dry-run")),
      `${name} must only ever dry-run`,
    );
  }
  for (const name of ["kv:configure", "deploy", "dev"]) {
    assert.ok(!scripts[name].includes("--dry-run"), `${name} must not use the dry-run path`);
    assert.ok(!scripts[name].includes(DRY_RUN_CONFIG), `${name} must not read ${DRY_RUN_CONFIG}`);
  }
  // The guard runs after generation and immediately before the real deploy.
  const deploy = scripts.deploy;
  const guard = deploy.indexOf("--verify-deployable");
  const realDeploy = deploy.search(/wrangler deploy -c wrangler\.local\.jsonc/);
  assert.ok(deploy.startsWith("npm run kv:configure &&"), "deploy must regenerate the deployable config first");
  assert.ok(guard > 0 && realDeploy > guard, "deploy must run --verify-deployable before wrangler deploy");
  assert.equal(scripts["kv:configure"], "node scripts/apply-kv-namespace-id.mjs");
});
