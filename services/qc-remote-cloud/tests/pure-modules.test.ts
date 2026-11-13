import test from "node:test";
import assert from "node:assert/strict";
import { schemaViolations, describeViolations } from "../src/action-schema.ts";
import { projectAction } from "../src/action-projection.ts";
import { sanitizePublicResult, REMOVED_IDENTIFIER_FIELDS } from "../src/public-result.ts";
import { decodeDeviceMessage } from "../src/device-messages.ts";
import relayProfile from "../../../contracts/public/qc-relay-profile.v1.json" with { type: "json" };

test("a closed object does not accept an inherited property name as its own", () => {
  const schema = { type: "object", properties: {}, additionalProperties: false };
  for (const name of ["constructor", "toString", "valueOf", "hasOwnProperty", "__proto__"]) {
    const value = JSON.parse(`{"${name}": 1}`);
    assert.equal(schemaViolations(schema, value).length, 1, name);
  }
  assert.deepEqual(schemaViolations(schema, {}), []);
});

test("required and declared properties are own properties only", () => {
  const schema = { type: "object", required: ["toString"], properties: { toString: { type: "integer" } } };
  assert.equal(schemaViolations(schema, {}).length, 1, "an inherited name does not satisfy required");
  assert.equal(schemaViolations(schema, { toString: "x" }).length, 1);
  assert.deepEqual(schemaViolations(schema, { toString: 1 }), []);
});

test("the validator enforces ranges, enums and unknown keywords instead of passing them", () => {
  assert.equal(schemaViolations({ type: "integer", minimum: 1, maximum: 5 }, 9).length, 1);
  assert.equal(schemaViolations({ enum: ["a", "b"] }, "c").length, 1);
  assert.ok(describeViolations(schemaViolations({ type: "integer", maximum: 5 }, 9, "tempo"))?.includes("tempo"));
});

test("action arguments are matched by own property, so inherited names are not arguments", () => {
  assert.throws(() => projectAction("set_tempo", JSON.parse("{\"constructor\": 1}")), /not accepted/);
  assert.throws(() => projectAction("set_tempo", {}), /required/);
  assert.throws(() => projectAction("set_tempo", Object.create({ bpm: 120, expected_tempo: 100, expected_preset_name: "P", expected_preset_identity: "i" })), /required/, "inherited arguments do not count");
  const valid = { bpm: 120, expected_tempo: 100, expected_preset_name: "P", expected_preset_identity: "i" };
  assert.equal(projectAction("set_tempo", valid).action.name, "set_tempo");
});

test("the identifier strip removes every published field at any depth and case, and withholds a backup payload", () => {
  const nested: Record<string, unknown> = { keep: 1 };
  for (const field of REMOVED_IDENTIFIER_FIELDS) nested[field.toUpperCase()] = "secret";
  const result = sanitizePublicResult({ outer: [{ inner: nested }], type: "backup", payload: "QUJD" }) as Record<string, unknown>;
  assert.deepEqual((result.outer as Array<{ inner: unknown }>)[0].inner, { keep: 1 });
  assert.equal(result.payload, undefined);
  assert.equal(result.payloadBytes, 4);
  assert.match(String(result.payloadWithheld), /deliberately not sent/);
});

test("device messages are bounded by UTF-8 bytes at the limit, without encoding small ones", () => {
  const limit = relayProfile.maxResultFrameBytes;
  const text = (units: number, filler = "a") => `{"k":"${filler.repeat(units - 8)}"}`;
  assert.deepEqual(Object.keys(decodeDeviceMessage(text(100))), ["k"]);
  // Under a third of the limit in code units can never exceed it in bytes.
  assert.deepEqual(Object.keys(decodeDeviceMessage(text(Math.floor(limit / 3) - 10, "€"))), ["k"]);
  // Above a third, three-byte characters decide it: 2.5 limit/3 euro signs is over the byte limit.
  assert.throws(() => decodeDeviceMessage(text(Math.floor(limit / 3) + 10, "€")), /Invalid message/);
  // The same unit count in single-byte characters is fine.
  assert.deepEqual(Object.keys(decodeDeviceMessage(text(Math.floor(limit / 3) + 10, "a"))), ["k"]);
  assert.throws(() => decodeDeviceMessage("x".repeat(limit + 1)), /Invalid message/);
  assert.throws(() => decodeDeviceMessage(new ArrayBuffer(4)), /Invalid message/);
  assert.throws(() => decodeDeviceMessage("[1]"), /Invalid message/);
});
