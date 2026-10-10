import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const read = (path) => readFileSync(resolve(root, path), "utf8");
const json = (path) => JSON.parse(read(path));
const load = (path) => import(pathToFileURL(resolve(root, path)).href);
const javaRoot = "apps/android/android/app/src/main/java/org/freevia/qcremote/";

function literal(expression) {
  const value = expression.trim().replace(/^&(?=\[)/, "").replace(/^\{/, "[").replace(/\}$/, "]")
    .replace(/\b(\d[\d_]*)[LuU](?:L)?\b/g, "$1").replace(/\b\d[\d_]*\b/g, (number) => number.replaceAll("_", ""))
    .replace(/,\s*\]/g, "]");
  return JSON.parse(value);
}
function rustConstants(text) {
  return new Map([...text.matchAll(/pub const ([A-Z][A-Z_0-9]*):[^=]+?=\s*([^;]+);/g)]
    .map((match) => [match[1], literal(match[2])]));
}
function javaConstants(text) {
  return new Map([...text.matchAll(/static final (?:int|long|String|boolean)(?:\[\])? ([A-Z][A-Z_0-9]*)\s*=\s*([^;]+);/g)]
    .map((match) => [match[1], literal(match[2])]));
}

test("USB profile values and cloud inner budgets match the Rust snapshot", async () => {
  const rust = rustConstants(read("packages/rust/quadcortex/src/profile.rs"));
  const java = javaConstants(read(`${javaRoot}QcUsbProfile.java`));
  assert.ok(rust.size > 40 && java.size > 40, "profile parser must cover the complete scalar/array table");
  for (const [name, value] of java) assert.deepEqual(value, rust.get(name), `Android ${name}`);
  const { INNER_BUDGETS_MS } = await load("services/qc-remote-cloud/src/device-deadlines.ts");
  assert.ok(Object.keys(INNER_BUDGETS_MS).length >= 7);
  for (const [name, value] of Object.entries(INNER_BUDGETS_MS)) assert.equal(value, rust.get(name), `cloud ${name}`);
});

test("relay public profile is identical in Java and Rust", () => {
  const profile = json("contracts/public/qc-relay-profile.v1.json");
  const rust = rustConstants(read("packages/rust/qc-relay-protocol/src/generated_profile.rs"));
  const java = javaConstants(read(`${javaRoot}GeneratedRelayProfile.java`));
  const nameOf = (name) => name.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toUpperCase();
  const entries = Object.entries(profile).filter(([name]) => !["version", "errorTexts", "removedIdentifierFields", "androidPingIntervalMs", "pairingOwnerPolicy"].includes(name));
  assert.ok(entries.length >= 20);
  for (const [name, value] of entries) {
    assert.deepEqual(rust.get(nameOf(name)), value, `Rust ${name}`);
    assert.deepEqual(java.get(nameOf(name)), value, `Java ${name}`);
  }
  assert.deepEqual(rust.get("REMOVED_IDENTIFIER_FIELDS"), profile.removedIdentifierFields);
  assert.equal(java.get("PING_INTERVAL_MS"), profile.androidPingIntervalMs);
  assert.ok(Object.keys(profile.errorTexts).length >= 7);
  for (const [name, value] of Object.entries(profile.errorTexts)) {
    assert.equal(rust.get(name), value, `Rust ${name}`);
    assert.equal(java.get(name), value, `Java ${name}`);
  }
});

test("pairing owner policy function snapshots preserve the contract rules and rejection boundaries", () => {
  const policy = json("contracts/public/qc-relay-profile.v1.json").pairingOwnerPolicy;
  assert.deepEqual(policy.rules, [
    { when: "different-known-owner", code: "PAIRING_OWNER_CHANGED" },
    { when: "missing-incoming-owner", code: "PAIRING_OWNER_UNVERIFIED" },
    { when: "missing-held-owner", code: "PAIRING_OWNER_UNKNOWN" },
  ]);
  assert.deepEqual(policy.rejectionHttpStatuses, [401, 403]);
  assert.deepEqual(policy.rejectionCloseCodes, [4001, 4003]);
  const rust = read("packages/rust/qc-relay-protocol/src/generated_profile.rs");
  const java = read(`${javaRoot}GeneratedRelayProfile.java`);
  const conditions = {
    "different-known-owner": ["!held.is_empty() && !incoming.is_empty() && !held.eq_ignore_ascii_case(incoming)", "!held.isEmpty() && !incoming.isEmpty() && !held.equalsIgnoreCase(incoming)"],
    "missing-incoming-owner": ["!held.is_empty() && incoming.is_empty()", "!held.isEmpty() && incoming.isEmpty()"],
    "missing-held-owner": ["already_paired && held.is_empty()", "alreadyPaired && held.isEmpty()"],
  };
  const compact = (value) => value.replace(/\s+/g, "");
  const rustRules = [...rust.matchAll(/if\s+([^{}]+)\{\s*return Some\("(PAIRING_OWNER_[A-Z]+)"\);\s*\}/g)]
    .map(([, condition, code]) => [compact(condition), code]);
  const javaRules = [...java.matchAll(/if \(([^\n]+)\) return "(PAIRING_OWNER_[A-Z]+)";/g)]
    .map(([, condition, code]) => [compact(condition), code]);
  assert.equal(rustRules.length, 3);
  assert.equal(javaRules.length, 3);
  assert.deepEqual(rustRules, policy.rules.map(({ when, code }) => [compact(conditions[when][0]), code]));
  assert.deepEqual(javaRules, policy.rules.map(({ when, code }) => [compact(conditions[when][1]), code]));
  assert.equal(policy.rejectedCredentialBypass, true);
  assert.match(rust, /if rejected\s*\{\s*return None;\s*\}/);
  assert.match(java, /if \(rejected\) return null;/);
  for (const [suffix, javaSuffix, field, argument] of [
    ["http", "Http", "rejectionHttpStatuses", "status"],
    ["close", "Close", "rejectionCloseCodes", "code"],
  ]) {
    const rustBody = rust.match(new RegExp(`pub fn pairing_rejection_${suffix}\\([^)]*\\)[^{]*\\{([^}]+)\\}`));
    const javaBody = java.match(new RegExp(`static boolean pairingRejection${javaSuffix}\\([^)]*\\) \\{([^}]+)\\}`));
    assert.ok(rustBody && javaBody, `${suffix} rejection functions exist`);
    assert.equal(compact(rustBody[1]), `[${policy[field].join(",")}].contains(&${argument})`);
    assert.equal(compact(javaBody[1]), `return${policy[field].map((value) => `${argument}==${value}`).join("||")};`);
  }
});

test("the wire message table matches the published names, excluding only sentinels", () => {
  const expected = Object.entries(json("references/public/cortex-protocol/message-types.json"))
    .filter(([, name]) => !["Undefined", "NumberOfMessageTypes"].includes(name)).map(([id, name]) => [Number(id), name]);
  assert.ok(expected.length >= 70);
  const source = read("packages/rust/quadcortex/src/message_registry.rs");
  const table = source.match(/pub const MESSAGE_TYPES:[\s\S]*?= \[([\s\S]*?)\n\];/);
  assert.ok(table);
  const actual = [...table[1].matchAll(/\((\d+), "([^"]+)"\)/g)].map(([, id, name]) => [Number(id), name]);
  assert.deepEqual(actual, expected);
});

test("device palette snapshots preserve display and wire colours as distinct tables", async () => {
  const colors = json("packages/typescript/qc-theme/src/colors.json");
  const generated = await load("packages/typescript/qc-core/src/device-reference/generated-device-colors.ts");
  assert.deepEqual(generated.QC_SCENE_COLORS, colors.scene);
  assert.deepEqual(generated.QC_PRESET_SLOT_COLORS, colors.presetSlot);
  const domain = read("packages/rust/quadcortex/src/domain.rs");
  for (const [name, values] of [["SCENE_COLORS", generated.QC_SCENE_COLORS], ["SCENE_COLOR_ARGB", generated.QC_SCENE_COLOR_ARGB]]) {
    const match = domain.match(new RegExp(`pub const ${name}: [^=]+ = &\\[([\\s\\S]*?)\\];`));
    assert.ok(match, name);
    assert.deepEqual(literal(`[${match[1]}]`), [...values], name);
  }
});

test("the typed client and positional invoker share every declared argument name and order", async () => {
  const manifest = json("contracts/public/gateway-methods.v1.json");
  const { GATEWAY_CLIENT_METHODS } = await load("packages/typescript/qc-client/src/generated-gateway-methods.ts");
  const source = read("packages/typescript/qc-client/src/generated-gateway-transport.ts");
  const signatures = new Map([...source.matchAll(/^  (\w+)\((.*)\): Promise<(.+)>;$/gm)].map(([, name, parameters, result]) => [name, { parameters, result }]));
  assert.ok(manifest.methods.length >= 110 && signatures.size >= 110);
  for (const method of manifest.methods) {
    const names = (method.args ?? []).map((arg) => typeof arg === "string" ? arg : arg.name);
    assert.deepEqual([...GATEWAY_CLIENT_METHODS[method.client].params], names, method.client);
    if (method.typescript.exposed === false) { assert.ok(!signatures.has(method.client)); continue; }
    const spec = method.typescript;
    assert.deepEqual(Object.keys(spec.parameters), names, method.client);
    assert.deepEqual(signatures.get(method.client), {
      parameters: names.map((name) => `${name}${spec.optionalParameters.includes(name) ? "?" : ""}: ${spec.parameters[name]}`).join(", "),
      result: spec.result,
    }, method.client);
  }
  assert.equal(signatures.size, manifest.methods.filter((method) => method.typescript.exposed !== false).length);
});

test("host policy snapshots agree on refusal markers and retained-state dispatch", () => {
  const policy = json("contracts/public/qc-host-policy.v1.json");
  const java = javaConstants(read(`${javaRoot}GeneratedHostPolicy.java`));
  const rust = read("packages/rust/quadcortex/src/generated_host_policy.rs");
  const shared = { DEVICE_SUSPENDED_MARKER: policy.deviceSuspendedMarker, PRESET_CATALOG_REFRESH_UNAVAILABLE: policy.presetCatalogRefreshUnavailable };
  for (const [name, value] of Object.entries(shared)) {
    assert.equal(java.get(name), value);
    assert.ok(rust.includes(`pub const ${name}: &str = ${JSON.stringify(value)};`));
  }
  assert.equal(java.get("UNCERTAIN_DEVICE_EFFECT_MARKER"), policy.uncertainDeviceEffectMarker);
  assert.deepEqual(java.get("SUSPENSION_CACHE_DISPATCH_KINDS"), policy.androidSuspensionCacheDispatchKinds);
  const toggles = Object.entries(json("contracts/public/qc-actions.v1.json").generalSettings.readOnlyBooleans);
  assert.ok(toggles.length > 0);
  for (const [name, rule] of toggles) assert.match(rust, new RegExp(`"${name}",\\s*${JSON.stringify(rule.reason)}`));
});

test("access tiers and result-kind snapshots agree across hosts", async () => {
  const contract = json("contracts/public/qc-actions.v1.json");
  const actions = await load("packages/typescript/qc-core/src/generated-actions.ts");
  assert.deepEqual([...actions.SHARED_QC_ACCESS_MODES], contract.accessModes);
  const rows = contract.actions;
  assert.ok(rows.length >= 110);
  const expected = {
    ALLOWED: ["system.status", ...new Set(rows.map(({ rpc }) => rpc))],
    READ_ONLY: ["system.status", ...new Set(rows.filter(({ classification }) => classification === "read").map(({ rpc }) => rpc))],
    PERFORMANCE: rows.filter(({ name }) => contract.accessTiers.performance.includes(name)).map(({ rpc }) => rpc),
    MODIFY: rows.filter(({ name }) => contract.accessTiers.modify.includes(name)).map(({ rpc }) => rpc),
  };
  const rust = read("packages/rust/qc-relay-client/src/generated_actions.rs");
  const java = read(`${javaRoot}GeneratedRemoteActions.java`);
  for (const [name, values] of Object.entries(expected)) {
    const rustTable = rust.match(new RegExp(`const ${name}: [^=]+ = &\\[([\\s\\S]*?)\\];`));
    const javaTable = java.match(new RegExp(`String\\[\\] ${name} = \\{([\\s\\S]*?)\\};`));
    assert.ok(rustTable && javaTable, name);
    assert.deepEqual(literal(`[${rustTable[1]}]`), values, `Rust ${name}`);
    assert.deepEqual(literal(`[${javaTable[1]}]`), values, `Java ${name}`);
  }
  const { GATEWAY_RESULT_KINDS } = await load("packages/typescript/qc-client/src/generated-gateway-methods.ts");
  const source = read("services/rust-mcp/src/generated_result_kinds.rs");
  const kinds = Object.fromEntries([...source.matchAll(/"([^"]+)" => Some\(ResultKind::(\w+)\)/g)].map(([, rpc, kind]) => [rpc, kind]));
  assert.deepEqual(kinds, GATEWAY_RESULT_KINDS);
});

test("the UI phase vocabulary and native aliases come from the public domain contract", async () => {
  const domain = json("contracts/public/qc-domain.v1.json");
  const generated = await load("packages/typescript/qc-client/src/generated-domain.ts");
  assert.deepEqual([...generated.QC_CONNECTION_PHASES], domain.connectionPhases);
  assert.deepEqual(generated.QC_BROKER_CONNECTION_PHASE_ALIASES, domain.brokerConnectionPhaseAliases);
  assert.ok(domain.connectionPhases.length >= 9);
  for (const phase of Object.values(domain.brokerConnectionPhaseAliases)) assert.ok(domain.connectionPhases.includes(phase));
});
