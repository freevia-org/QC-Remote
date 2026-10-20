import assert from "node:assert/strict";
import test from "node:test";
import type { GatewayTransport } from "@qc-remote/client";
import { bindMobileTarget, type MobileProviderOrigin, type MobileTargetKind } from "./mobile-target-binding.ts";

const gateway = (name: string) => ({ currentSnapshot: async () => ({ presetName: name }), renameCurrentPreset: async () => name }) as unknown as GatewayTransport;

test("host demo reads and edits stay on the paired host without opening a phone-local demo", async () => {
  let localLoads = 0;
  const host = gateway("host demo");
  const binding = bindMobileTarget({ origin: "paired-host", phase: "ready", kind: "demo", sessionIdentity: "host-generation-7", providers: {
    "paired-host": () => host,
    "local-demo": () => { localLoads++; throw new Error("must not open local demo"); }
  } })!;
  assert.equal(binding.kind, "demo");
  assert.equal(binding.origin, "paired-host");
  assert.equal(binding.gateway, host);
  assert.equal((await binding.gateway.currentSnapshot()).presetName, "host demo");
  assert.equal(await (binding.gateway.renameCurrentPreset as any)(), "host demo");
  assert.equal(localLoads, 0);
});

test("only an explicit local-demo origin selects the existing local demo gateway", () => {
  const local = gateway("local");
  const binding = bindMobileTarget({ origin: "local-demo", phase: "ready", kind: "demo", sessionIdentity: "local-session", providers: { "local-demo": () => local, "paired-host": () => { throw new Error("must not select host"); } } })!;
  assert.equal(binding.gateway, local);
  assert.throws(() => bindMobileTarget({ origin: "local-demo", phase: "ready", kind: "physical", sessionIdentity: "session", providers: {} }), /cannot represent physical/);
});

test("unavailable or syncing targets never load a provider or silently fall back", () => {
  for (const phase of ["disconnected", "needs-attention", "opening", "syncing"] as const) {
    assert.equal(bindMobileTarget({ origin: "paired-host", phase, kind: "demo", sessionIdentity: "generation", providers: { "paired-host": () => { throw Error("not ready"); }, "local-demo": () => { throw Error("no fallback"); } } }), undefined);
  }
  assert.throws(() => bindMobileTarget({ origin: "paired-host", phase: "ready", kind: "demo", sessionIdentity: "generation", providers: { "local-demo": () => gateway("wrong") } }), /provider is unavailable/);
});

test("provider, target kind and generation changes each produce a different operation-fence key", () => {
  const keys = new Set<string>();
  for (const [origin, kind, sessionIdentity] of [
    ["paired-host", "demo", "g1"], ["paired-host", "physical", "g2"], ["paired-host", "demo", "g3"],
    ["local-demo", "demo", "g3"], ["native-gateway", "physical", "g2"]
  ] as const) {
    const providers = Object.fromEntries(["paired-host", "local-demo", "native-gateway"].map(origin => [origin, () => gateway(origin)])) as Record<MobileProviderOrigin, () => GatewayTransport>;
    const binding = bindMobileTarget({ origin, phase: "ready", kind, sessionIdentity, providers })!;
    assert.ok(!keys.has(binding.key)); keys.add(binding.key);
  }
});

test("missing session identity or unknown target kind fail before dispatch", () => {
  let reads = 0;
  const providers = { "paired-host": () => { reads++; return gateway("host"); } };
  assert.throws(() => bindMobileTarget({ origin: "paired-host", phase: "ready", kind: "demo", sessionIdentity: "", providers }), /identity is unavailable/);
  assert.throws(() => bindMobileTarget({ origin: "paired-host", phase: "ready", kind: "future" as MobileTargetKind, sessionIdentity: "g1", providers }), /kind is unavailable/);
  assert.equal(reads, 0);
});
