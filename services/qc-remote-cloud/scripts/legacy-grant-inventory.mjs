#!/usr/bin/env node
// Operator tool: inspect, and only with explicit authorization revoke, the
// legacy AI-app grants of ONE remote account (account-deletion.ts, "legacy
// reconciliation"). It is never run on deploy, by the Worker, or when the
// deletion page is enabled.
//
//   node scripts/legacy-grant-inventory.mjs --subject <google-sub>
//       DRY RUN (the default). Lists that account's `grant:access-<sub>:*` and
//       `token:access-<sub>:*` keys through the Cloudflare API and prints, per
//       grant, a short hash of the key, whether `expiresAt` and
//       `metadata.accountGeneration` are present, and `expiresAt`; then the
//       latest `expiresAt` and how many grants have none. Prints nothing
//       personal and writes nothing.
//
//   ... --execute --authorization <reference>
//       Deletes exactly the keys the dry run listed for that one account
//       (tokens first, then grants), and nothing else. Requires both flags;
//       `--authorization` records who approved it. Do not run without that
//       explicit, later approval.
//
// Environment (read from the shell only): CLOUDFLARE_API_TOKEN,
// CLOUDFLARE_ACCOUNT_ID, QCRC_OAUTH_KV_NAMESPACE_ID.
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

export function parseArguments(argv) {
  const options = { subject: null, execute: false, authorization: null };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--subject") options.subject = argv[++index] ?? null;
    else if (argument === "--execute") options.execute = true;
    else if (argument === "--authorization") options.authorization = argv[++index] ?? null;
    else throw new Error(`Unknown argument: ${argument}`);
  }
  if (!options.subject || !options.subject.trim()) throw new Error("--subject <google-sub> is required: this tool works on one account at a time");
  if (options.execute && !options.authorization) throw new Error("--execute requires --authorization <reference> for the explicit approval");
  if (!options.execute && options.authorization) throw new Error("--authorization is only meaningful with --execute");
  return options;
}

export function accountPrefixes(subject) {
  const userId = `access-${encodeURIComponent(subject)}`;
  return { grant: `grant:${userId}:`, token: `token:${userId}:` };
}

const keyHash = (name) => createHash("sha256").update(name).digest("hex").slice(0, 12);

/** `api` is { list(prefix, cursor) -> {keys:[{name}], cursor?}, get(name) -> string|null, delete(name) }. */
export async function inventory(api, subject) {
  const prefixes = accountPrefixes(subject);
  const collect = async (prefix) => {
    const names = [];
    let cursor;
    do {
      const page = await api.list(prefix, cursor);
      names.push(...page.keys.map((key) => key.name));
      cursor = page.cursor || undefined;
    } while (cursor);
    return names;
  };
  const grants = await collect(prefixes.grant);
  const tokens = await collect(prefixes.token);
  const rows = [];
  for (const name of grants) {
    // A failed read must fail the report: recording it as a grant without an
    // expiry would overstate "grants without expiry" in what a reviewer approves.
    // Only a value that is missing or not JSON counts as having no readable expiry.
    const text = await api.get(name);
    let parsed = null;
    try { parsed = JSON.parse(text ?? "null"); } catch { parsed = null; }
    rows.push({
      key: keyHash(name),
      expiresAt: typeof parsed?.expiresAt === "number" ? parsed.expiresAt : null,
      hasGeneration: typeof parsed?.metadata?.accountGeneration === "number",
    });
  }
  const expiries = rows.map((row) => row.expiresAt).filter((value) => value !== null);
  return {
    grants: rows,
    tokenCount: tokens.length,
    latestExpiresAt: expiries.length > 0 ? Math.max(...expiries) : null,
    grantsWithoutExpiry: rows.filter((row) => row.expiresAt === null).length,
    keys: { grants, tokens },
  };
}

export async function run(argv, api, print = (line) => console.log(line)) {
  const options = parseArguments(argv);
  const report = await inventory(api, options.subject);
  const { keys, ...printable } = report;
  print(JSON.stringify({ mode: options.execute ? "execute" : "dry-run", ...printable }, null, 2));
  if (!options.execute) return { deleted: 0 };
  let deleted = 0;
  let failedAt = null;
  try {
    for (const name of [...keys.tokens, ...keys.grants]) {
      try { await api.delete(name); } catch (error) { failedAt = keyHash(name); throw error; }
      deleted += 1;
    }
  } finally {
    // Printed even when a delete failed, so the operator knows how far it got
    // and which key (by its short hash) stopped it.
    print(JSON.stringify({ deleted, failedAt, authorization: options.authorization }));
  }
  return { deleted };
}

function cloudflareApi(environment = process.env) {
  const token = environment.CLOUDFLARE_API_TOKEN;
  const account = environment.CLOUDFLARE_ACCOUNT_ID;
  const namespace = environment.QCRC_OAUTH_KV_NAMESPACE_ID;
  if (!token || !account || !namespace) throw new Error("CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID and QCRC_OAUTH_KV_NAMESPACE_ID are required");
  const base = `https://api.cloudflare.com/client/v4/accounts/${account}/storage/kv/namespaces/${namespace}`;
  const headers = { authorization: `Bearer ${token}` };
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  /** Every request has a deadline; 429, 5xx and network errors are retried with backoff. */
  async function request(url, init = {}, { attempts = 4, timeoutMs = 20_000, wait = sleep } = {}) {
    let lastError;
    for (let attempt = 0; attempt < attempts; attempt += 1) {
      if (attempt > 0) await wait(500 * 2 ** (attempt - 1));
      try {
        const response = await fetch(url, { ...init, headers, signal: AbortSignal.timeout(timeoutMs) });
        if (response.status === 429 || response.status >= 500) { lastError = new Error(`Cloudflare API answered ${response.status}`); continue; }
        return response;
      } catch (error) { lastError = error; }
    }
    throw lastError;
  }
  return {
    async list(prefix, cursor) {
      const url = new URL(`${base}/keys`);
      url.searchParams.set("prefix", prefix);
      if (cursor) url.searchParams.set("cursor", cursor);
      const body = await (await request(url)).json();
      if (!body.success) throw new Error("Cloudflare KV list failed");
      return { keys: body.result, cursor: body.result_info?.cursor || undefined };
    },
    async get(name) {
      const response = await request(`${base}/values/${encodeURIComponent(name)}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new Error(`Cloudflare KV read failed (${response.status})`);
      return response.text();
    },
    async delete(name) {
      const response = await request(`${base}/values/${encodeURIComponent(name)}`, { method: "DELETE" });
      if (!response.ok) throw new Error("Cloudflare KV delete failed");
    },
  };
}

/** True when this file is the script being run, however it was reached (a
 * symlink, a junction, a different drive-letter case). */
export function isEntryPoint(argv1 = process.argv[1], moduleUrl = import.meta.url) {
  if (!argv1) return false;
  try { return realpathSync(argv1) === realpathSync(fileURLToPath(moduleUrl)); } catch { return false; }
}

if (isEntryPoint()) {
  run(process.argv.slice(2), cloudflareApi()).catch((error) => { console.error(error.message); process.exit(1); });
}
