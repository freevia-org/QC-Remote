#!/usr/bin/env node
// Generates a wrangler config from wrangler.jsonc (committed) by substituting a
// Cloudflare KV namespace id for the template token, so the real id never lands
// in source control. See README.md.
//
//   (no flag)            wrangler.local.jsonc, for deploy and dev. Requires
//                        QCRC_OAUTH_KV_NAMESPACE_ID to hold a real id and
//                        refuses the dry-run placeholder below.
//   --dry-run            wrangler.check.jsonc, for check and deploy:dry. Uses
//                        QCRC_OAUTH_KV_NAMESPACE_ID when it is set, otherwise
//                        the labelled all-zero placeholder, so a fork PR or a
//                        new repository without the secret can still type-check
//                        and dry-run the Worker.
//   --verify-deployable  writes nothing; exits non-zero unless
//                        wrangler.local.jsonc exists and binds a real id. The
//                        deploy script runs it immediately before
//                        `wrangler deploy`.
//
// The placeholder goes to its own file rather than wrangler.local.jsonc on
// purpose: other tooling (for example, scripts that provision new checkouts)
// may copy whatever 32-hex id it finds in a wrangler.local.jsonc into them, and
// the all-zero id is syntactically valid. Keeping it out of the deployable file
// means a dry run can never seed it anywhere a deploy reads from.
import { existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const ENV_VAR = "QCRC_OAUTH_KV_NAMESPACE_ID";
export const TEMPLATE_TOKEN = "REPLACE_WITH_QCRC_OAUTH_KV_NAMESPACE_ID";
// Labelled placeholder for dry runs only: the shape wrangler accepts (32 hex
// digits), a value no real Cloudflare namespace has. Never deployable.
export const DRY_RUN_PLACEHOLDER_ID = "00000000000000000000000000000000";
export const DEPLOY_CONFIG = "wrangler.local.jsonc";
export const DRY_RUN_CONFIG = "wrangler.check.jsonc";
const ID_PATTERN = /^[0-9a-f]{32}$/i;

const DEFAULT_SERVICE_DIR = dirname(dirname(fileURLToPath(import.meta.url)));

export function isNamespaceId(value) {
  return typeof value === "string" && ID_PATTERN.test(value);
}

export function isPlaceholderId(value) {
  return typeof value === "string" && value.trim().toLowerCase() === DRY_RUN_PLACEHOLDER_ID;
}

// Decides which id a mode may use. Returns { id, placeholder } or { error }.
export function resolveNamespaceId({ env = process.env, dryRun = false } = {}) {
  const raw = env[ENV_VAR];
  const value = typeof raw === "string" ? raw.trim() : "";
  if (!value) {
    if (dryRun) return { id: DRY_RUN_PLACEHOLDER_ID, placeholder: true };
    return {
      error:
        `Missing ${ENV_VAR}.\n` +
        "Set it in your shell environment before running this script - a .env file\n" +
        "cannot be used here, see above. In CI, set it as the\n" +
        "QCRC_OAUTH_KV_NAMESPACE_ID repository secret. See README.md.",
    };
  }
  if (!isNamespaceId(value)) {
    return { error: `${ENV_VAR} is set but is not a 32-digit hexadecimal Cloudflare KV namespace id.` };
  }
  if (isPlaceholderId(value) && !dryRun) {
    return {
      error:
        `${ENV_VAR} holds the all-zero dry-run placeholder, which is not a real KV namespace.\n` +
        "Refusing to generate a deployable config with it. Set the real namespace id.",
    };
  }
  return { id: value, placeholder: isPlaceholderId(value) };
}

// The ids a generated config binds, from every "id" field in it.
function boundIds(text) {
  return [...text.matchAll(/"id"\s*:\s*"([^"]*)"/g)].map((match) => match[1].trim());
}

function hasRequiredObservabilityConfig(configPath, serviceDir) {
  try {
    let unstable_readConfig;
    for (const baseDir of new Set([serviceDir, DEFAULT_SERVICE_DIR, process.cwd()])) {
      try {
        ({ unstable_readConfig } = createRequire(join(baseDir, "package.json"))("wrangler"));
        break;
      } catch {
        // A directory-junction invocation may use a temporary script copy;
        // fall back to the actual service checkout or the caller's workspace.
      }
    }
    if (typeof unstable_readConfig !== "function") return false;
    const config = unstable_readConfig({ config: configPath });
    return config.observability?.redact_query_string === true && config.observability?.head_sampling_rate === 0.01;
  } catch {
    return false;
  }
}

// Null when `text` is a deployable config, otherwise the reason it is not.
export function deployabilityProblem(text) {
  if (text.includes(TEMPLATE_TOKEN)) return `it still holds the template token ${TEMPLATE_TOKEN}`;
  const ids = boundIds(text);
  if (ids.length === 0) return "it binds no KV namespace id";
  if (ids.some(isPlaceholderId)) return `it binds the dry-run placeholder id ${DRY_RUN_PLACEHOLDER_ID}`;
  const malformed = ids.filter((id) => !isNamespaceId(id));
  if (malformed.length > 0) return "it binds an id that is not a 32-digit hexadecimal KV namespace id";
  return null;
}

export function main(argv = process.argv.slice(2), { env = process.env, serviceDir = DEFAULT_SERVICE_DIR, log = console.log, error = console.error } = {}) {
  const known = new Set(["--dry-run", "--verify-deployable"]);
  const unknown = argv.filter((arg) => !known.has(arg));
  if (unknown.length > 0 || argv.length > 1) {
    error("usage: node scripts/apply-kv-namespace-id.mjs [--dry-run | --verify-deployable]");
    return 2;
  }
  const dryRun = argv.includes("--dry-run");
  const deployConfigPath = join(serviceDir, DEPLOY_CONFIG);

  if (argv.includes("--verify-deployable")) {
    if (!existsSync(deployConfigPath)) {
      error(`${deployConfigPath} does not exist. Run \`npm run kv:configure\` with ${ENV_VAR} set.`);
      return 1;
    }
    const problem = deployabilityProblem(readFileSync(deployConfigPath, "utf8"));
    if (problem) {
      error(`Refusing to deploy with ${deployConfigPath}: ${problem}.`);
      return 1;
    }
    if (!hasRequiredObservabilityConfig(deployConfigPath, serviceDir)) {
      error(`Refusing to deploy with ${deployConfigPath}: query-string redaction and 1% observability sampling are required.`);
      return 1;
    }
    log(`${deployConfigPath} binds a real KV namespace id.`);
    return 0;
  }

  // A .env here cannot work, so it is refused rather than read.
  //
  // This script used to fall back to reading .env itself, and README.md told you
  // to create one. Reading it worked; every wrangler call in the same npm script
  // then broke, several steps later and in another vocabulary. Wrangler loads .env
  // into the Worker's `vars`, so `wrangler types` emits
  // `QCRC_OAUTH_KV_NAMESPACE_ID: string;` into `Cloudflare.Env` and adds it to
  // `ProcessEnv`; worker-configuration.d.ts is committed, `npm run check` runs
  // `wrangler types --check`, and the check fails on two lines the committed file
  // does not have. Measured on wrangler 4.130.0.
  //
  // The id was never a Worker variable to begin with -- it is a build-time
  // substitution into a KV binding id, which is what this script performs -- so
  // .env is the wrong channel by construction and not merely an unlucky one.
  // Regenerating the committed types with the variable present would also make the
  // chain pass; it bakes a local-only build input into the Worker's public type
  // surface, which is worse.
  //
  // This script is the first step of check, deploy:dry, deploy and dev, so
  // refusing here is the earliest point at which anything can say so, and it says
  // it in the terms the developer can act on.
  const dotEnvPath = join(serviceDir, ".env");
  if (existsSync(dotEnvPath)) {
    error(
      `${dotEnvPath} exists, and wrangler cannot be run beside it.\n` +
        "Wrangler loads .env into the Worker's vars, so `wrangler types --check`\n" +
        "then fails against the committed worker-configuration.d.ts. Delete the\n" +
        `file and put ${ENV_VAR} in your shell environment instead. See README.md.`,
    );
    return 1;
  }

  const resolved = resolveNamespaceId({ env, dryRun });
  if (resolved.error) {
    error(resolved.error);
    return 1;
  }

  const templatePath = join(serviceDir, "wrangler.jsonc");
  const template = readFileSync(templatePath, "utf8");
  if (!template.includes(TEMPLATE_TOKEN)) {
    error(`Expected placeholder "${TEMPLATE_TOKEN}" was not found in ${templatePath}. Has wrangler.jsonc changed shape?`);
    return 1;
  }
  if (!hasRequiredObservabilityConfig(templatePath, serviceDir)) {
    error("Refusing to generate a Wrangler config without query-string redaction and 1% observability sampling.");
    return 1;
  }

  const outputPath = join(serviceDir, dryRun ? DRY_RUN_CONFIG : DEPLOY_CONFIG);
  writeFileSync(outputPath, template.split(TEMPLATE_TOKEN).join(resolved.id));
  if (resolved.placeholder) {
    log(
      `NOTE: no real ${ENV_VAR} was supplied, so ${outputPath} binds the DRY-RUN PLACEHOLDER KV id ` +
        `${DRY_RUN_PLACEHOLDER_ID}. It is good for type checks and \`wrangler deploy --dry-run\` only; ` +
        "`npm run deploy` refuses it.",
    );
  }
  log(`Wrote ${outputPath}`);
  return 0;
}

// Compared by real path: other tooling may invoke this script through a
// directory junction or symlink, where argv[1] names the link and
// import.meta.url the target, so a plain resolve() never matches and the
// script would silently do nothing.
function isEntryPoint() {
  if (!process.argv[1]) return false;
  try {
    return realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

if (isEntryPoint()) {
  process.exitCode = main();
}
