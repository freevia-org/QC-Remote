import assert from "node:assert/strict";
import { readFile, writeFile, mkdir, realpath } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const contractPath = "contracts/public/broker-tray-surface.v1.json";
export const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const generatorPath = "services/device-broker/scripts/generate-broker-tray-surface.mjs";
const identifier = /^[A-Za-z][A-Za-z0-9_]*$/;
const types = new Set(["String", "u16", "u32", "u64", "Option<u64>"]);
const unique = (rows, key) => assert.equal(new Set(rows.map(row => row[key])).size, rows.length, `Duplicate ${key}`);

export function validateContract(contract) {
  assert.equal(contract.schemaVersion, 1, "Unsupported broker tray schema version");
  const descriptor = contract.descriptor;
  assert.ok(identifier.test(descriptor.writerName) && identifier.test(descriptor.readerName), "Invalid descriptor name");
  assert.ok(Array.isArray(descriptor.fields) && descriptor.fields.length > 0, "Descriptor fields are required");
  unique(descriptor.fields, "rust"); unique(descriptor.fields, "wire");
  for (const field of descriptor.fields) {
    assert.ok(identifier.test(field.rust) && identifier.test(field.wire), "Invalid descriptor field");
    assert.ok(types.has(field.writerType) && types.has(field.readerType), "Unsupported descriptor field type");
    assert.ok(field.readerDefault === undefined || field.readerDefault === true, "readerDefault must be true or absent");
    assert.ok(!field.readerDefault || field.readerType.startsWith("Option<"), "Defaulted reader fields must be optional");
  }
  for (const [name, writerType] of [["gateway_api_version", "u64"], ["process_created_at", "Option<u64>"]]) {
    const field = descriptor.fields.find(field => field.rust === name);
    assert.ok(field?.readerType === "Option<u64>" && field.readerDefault === true && field.writerType === writerType,
      `${name} must preserve older-descriptor compatibility`);
  }
  const marker = contract.releaseMarker;
  assert.equal(marker?.name, "BrokerReleaseMarker", "Release marker name must remain shared by both consumers");
  assert.deepEqual(marker.pathScheme, {
    base: "descriptor-with-extension", prefix: "released-", instanceEncoding: "utf8-hex-lower", suffix: ".json",
  }, "Release marker path must preserve each generation and hex-encode its bytes without path separators");
  assert.deepEqual(marker.fields, [
    { rust: "instance_id", wire: "instanceId", type: "String" },
    { rust: "forced", wire: "forced", type: "bool" },
  ], "Release marker must contain only the required generation identifier and forced flag; no credentials or timestamps");
  assert.deepEqual(contract.roles.map(role => role.rust), ["Participant", "Observer"], "Connection roles must preserve cooperative participant/observer semantics");
  assert.equal(contract.roles[0].default, true, "Absent role must remain participant");
  assert.equal(contract.roles[1].idlePolicy, true, "Observer must carry its declared idle policy");
  unique(contract.roles, "wire");
  for (const role of contract.roles) assert.match(role.wire, /^[a-z][a-zA-Z0-9]*$/);
  assert.deepEqual(contract.syncingPhases, ["handshaking", "syncing"], "Handshake and synchronization must be visible before Ready");
  const readings = [...contract.coarseReadings, ...contract.attachedReadings];
  unique(readings, "rust");
  for (const reading of readings) {
    assert.ok(identifier.test(reading.rust) && typeof reading.label === "string" && reading.label.length > 0, "Invalid tray reading");
  }
  assert.deepEqual(contract.coarseReadings.map(reading => reading.rust), ["BrokerNotRunning", "BrokerRunning"]);
  assert.deepEqual(contract.attachedReadings.map(reading => reading.rust), ["Disconnected", "Ready", "Syncing", "NoDevicePresent"], "Attached reading priority must be explicit");
  assert.deepEqual(contract.attachedReadings.map(reading => reading.when), [
    { phase: "disconnected" }, { connected: true, synchronized: true }, { connected: true }, {},
  ], "Ready requires both connected and synchronized; disconnected takes priority");
  return contract;
}

const quote = value => JSON.stringify(value);
function descriptorSource(contract, reader) {
  const name = contract.descriptor[reader ? "readerName" : "writerName"];
  // Debug is written by hand, not derived: the descriptor carries the broker's
  // bearer token, and a derived Debug prints it into any log, panic message or
  // error that formats the descriptor with {:?}.
  const derives = reader ? "Clone, serde::Deserialize, PartialEq, Eq" : "serde::Serialize";
  const fields = contract.descriptor.fields.map(field => [
    ...(field.description ? [`    /// ${field.description}`] : []),
    `    #[serde(rename = ${quote(field.wire)})]`,
    ...(reader && field.readerDefault ? ["    #[serde(default)]"] : []),
    `    pub ${field.rust}: ${field[reader ? "readerType" : "writerType"]},`,
  ].join("\n")).join("\n");
  const debugFields = contract.descriptor.fields.map(field => field.wire === "bearerToken"
    ? `            .field(${quote(field.rust)}, &"<redacted>")`
    : `            .field(${quote(field.rust)}, &self.${field.rust})`).join("\n");
  const debug = `impl std::fmt::Debug for ${name} {\n    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        formatter\n            .debug_struct(${quote(name)})\n${debugFields}\n            .finish()\n    }\n}\n`;
  return `#[derive(${derives})]\npub struct ${name} {\n${fields}\n}\n\n${debug}`;
}
function rolesSource(contract) {
  const declarations = contract.roles.map(role => role.idlePolicy
    ? `    ${role.rust} {\n        idle_policy_minutes: Option<u32>,\n    },`
    : `${role.default ? "    #[default]\n" : ""}    ${role.rust},`).join("\n");
  const names = contract.roles.map(role => `            Self::${role.rust}${role.idlePolicy ? " { .. }" : ""} => ${quote(role.wire)},`).join("\n");
  const parse = contract.roles.map(role => `            ${quote(role.wire)} => Some(Self::${role.rust}${role.idlePolicy ? " {\n                idle_policy_minutes,\n            }" : ""}),`).join("\n");
  return `#[derive(Clone, Debug, Default, PartialEq)]\npub enum ConnectionRole {\n${declarations}\n}\n\nimpl ConnectionRole {\n    pub fn wire_name(&self) -> &'static str {\n        match self {\n${names}\n        }\n    }\n\n    pub fn from_wire(role: &str, idle_policy_minutes: Option<u32>) -> Option<Self> {\n        match role {\n${parse}\n            _ => None,\n        }\n    }\n}\n`;
}
function releaseMarkerSource(contract) {
  const { name, pathScheme, fields } = contract.releaseMarker;
  const declarations = fields.map(field => `    #[serde(rename = ${quote(field.wire)})]\n    pub ${field.rust}: ${field.type},`).join("\n");
  const expectedPath = instance => `data/device-broker.${pathScheme.prefix}${Buffer.from(instance, "utf8").toString("hex")}${pathScheme.suffix}`;
  const generationWire = fields.find(field => field.rust === "instance_id").wire;
  const forcedWire = fields.find(field => field.rust === "forced").wire;
  return `// The tray imports this shared module but does not consume release markers.
#[allow(dead_code)]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ${name} {
${declarations}
}

#[allow(dead_code)]
impl ${name} {
    pub fn path_for(descriptor: &std::path::Path, instance_id: &str) -> std::path::PathBuf {
        let generation = instance_id
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        descriptor.with_extension(format!(${quote(`${pathScheme.prefix}{generation}${pathScheme.suffix}`)}))
    }
}

#[cfg(test)]
mod release_marker_tests {
    use super::${name};

    #[test]
    fn release_marker_wire_round_trip_and_generation_path() {
        for forced in [false, true] {
            let marker = ${name} {
                instance_id: "generation-123".into(),
                forced,
            };
            let wire = serde_json::to_value(&marker).unwrap();
            assert_eq!(
                wire,
                serde_json::json!({${quote(generationWire)}: "generation-123", ${quote(forcedWire)}: forced})
            );
            assert_eq!(
                serde_json::from_value::<${name}>(wire).unwrap(),
                marker
            );
        }
        for descriptor in ["data/device-broker.json", "data/device-broker"] {
            assert_eq!(
                ${name}::path_for(std::path::Path::new(descriptor), "ab"),
                std::path::PathBuf::from(${quote(expectedPath("ab"))})
            );
        }
        let descriptor = std::path::Path::new("data/device-broker.json");
        assert_ne!(
            ${name}::path_for(descriptor, "ab"),
            ${name}::path_for(descriptor, "ac")
        );
        let hostile = ${name}::path_for(descriptor, "../other/☃");
        assert_eq!(
            hostile,
            std::path::PathBuf::from(${quote(expectedPath("../other/☃"))})
        );
        assert_eq!(hostile.parent(), descriptor.parent());
        assert_eq!(
            ${name}::path_for(descriptor, "..\\\\other"),
            std::path::PathBuf::from(${quote(expectedPath("..\\other"))})
        );
        for invalid in [
            serde_json::json!({${quote(generationWire)}: "generation-123"}),
            serde_json::json!({${quote(forcedWire)}: true}),
            serde_json::json!({${quote(generationWire)}: "generation-123", ${quote(forcedWire)}: "true"}),
        ] {
            assert!(serde_json::from_value::<${name}>(invalid).is_err());
        }
    }
}
`;
}
function readingsSource(contract, inProcess = false) {
  const readings = [...contract.coarseReadings, ...contract.attachedReadings];
  // The broker draws its own icon, so it is always running: it never reports
  // the two coarse readings and does not need every label.
  const allow = inProcess ? "#[allow(dead_code)]\n" : "";
  const note = inProcess ? "// The in-process tray never reports the coarse readings.\n" : "";
  const variants = readings.map(reading => `    ${reading.rust},`).join("\n");
  const labels = readings.map(reading => `            Self::${reading.rust} => ${quote(reading.label)},`).join("\n");
  const condition = when => Object.entries(when).map(([key, value]) => key === "phase" ? `phase == ${quote(value)}` : key).join(" && ");
  const conditions = contract.attachedReadings.map((reading, index) => Object.keys(reading.when).length
    ? `        ${index ? "} else " : ""}if ${condition(reading.when)} {\n            Self::${reading.rust}`
    : `        } else {\n            Self::${reading.rust}\n        }`).join("\n");
  return `${note}${allow}#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub enum TrayReading {\n${variants}\n}\n\n${allow}impl TrayReading {\n    pub fn label(self) -> &'static str {\n        match self {\n${labels}\n        }\n    }\n\n    pub fn from_phase(phase: &str, connected: bool, synchronized: bool) -> Self {\n${`        if matches!(phase, ${contract.syncingPhases.map(quote).join(" | ")}) {\n            return Self::Syncing;\n        }`}\n${conditions}\n    }\n}\n`;
}
export function generateSources(input) {
  const contract = validateContract(input);
  const header = `// Generated by ${generatorPath}. Do not edit.\n// Contract: ${contractPath}\n`;
  return new Map([
    ["services/device-broker/src/generated_broker_surface.rs", header + descriptorSource(contract, false) + "\n" + descriptorSource(contract, true) + "\n" + releaseMarkerSource(contract) + "\n" + rolesSource(contract) + "\n" + readingsSource(contract, true)],
    ["apps/windows/src-tauri/src/generated_broker_surface.rs", header + descriptorSource(contract, true) + "\n" + releaseMarkerSource(contract) + "\n" + rolesSource(contract)],
  ]);
}
export async function checkOutputs(root, outputs) {
  for (const [relative, expected] of outputs) {
    let actual;
    try { actual = await readFile(resolve(root, relative), "utf8"); } catch { throw new Error(`Missing generated broker surface: ${relative}`); }
    if (actual.replaceAll("\r\n", "\n") !== expected) throw new Error(`Generated broker surface drift: ${relative}; run node ${generatorPath} --write`);
  }
}
export async function main(args = process.argv.slice(2), root = repositoryRoot) {
  if (args.some(arg => !["--check", "--write"].includes(arg)) || (args.includes("--check") && args.includes("--write"))) throw new Error("Use --check or --write");
  const outputs = generateSources(JSON.parse(await readFile(resolve(root, contractPath), "utf8")));
  if (!args.includes("--write")) await checkOutputs(root, outputs);
  else for (const [relative, content] of outputs) {
    const path = resolve(root, relative); await mkdir(dirname(path), { recursive: true }); await writeFile(path, content);
  }
  console.log(`Broker tray surface: ${outputs.size} generated consumers ${args.includes("--write") ? "written" : "verified"}`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(await realpath(resolve(process.argv[1]))).href) await main();
