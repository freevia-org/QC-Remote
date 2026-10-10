import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import type { DeviceActionResult, PresetSnapshot } from "../../qc-client/src/index.ts";
import { createMemoryVirtualDeviceStorage, createVirtualGatewayTransport, VirtualDeviceStore } from "../src/virtual-gateway.ts";
import type { VirtualModelCatalogue } from "../src/virtual-device-defaults.ts";

// The real hosts report a recall, bank navigation or reload as the address they
// observed in a position update, with no preset body bound to it: accepted, not
// verified, accepted_unverified, and an observation of kind
// preset_address_observed. The virtual gateway is practice on the real UI, so
// it reports the same shape, and its attached snapshot still moves the UI.
const catalogue = JSON.parse(
  readFileSync(new URL("../../../../docs/reference/qc-live-parameter-scales.json", import.meta.url), "utf8")
) as VirtualModelCatalogue;

async function openVirtual() {
  let session = 0;
  const store = await VirtualDeviceStore.open({
    storage: createMemoryVirtualDeviceStorage(), catalogue, now: () => 0, newSessionId: () => `session-${++session}`
  });
  return createVirtualGatewayTransport(store);
}

/** The shared rule: an unverified detail says "verified" only inside "not verified" or "unverified". */
function claimsVerification(detail: string): boolean {
  return detail.toLowerCase().replaceAll("not verified", "").replaceAll("unverified", "").includes("verified");
}

function assertAddressObserved(result: DeviceActionResult, snapshot: PresetSnapshot, reload: boolean) {
  assert.equal(claimsVerification(result.detail), false, `an unverified result claims verification: ${result.detail}`);
  const address = `${snapshot.setlistKey.replace(/\/+$/, "")} position ${snapshot.presetPosition}`;
  const expected = {
    accepted: true,
    verified: false,
    verification: "accepted_unverified",
    observation: {
      kind: "preset_address_observed", setlistKey: snapshot.setlistKey, position: snapshot.presetPosition,
      address: "observed", body: "unbound", causallyCorrelated: false,
      ...(reload ? { dirty: false } : {})
    },
    detail: reload
      ? `Active preset address resent; the host observed a position update for ${address} and a clean flag, which do not prove this request caused them, and no preset body can be bound to this reload, so the discard is not verified`
      : `Preset change sent; the host observed a position update for ${address}, which does not prove this request caused it, and no preset body can be bound to that address, so the loaded preset's contents are not verified`
  };
  const { snapshot: attached, ...rest } = result;
  assert.deepEqual(rest, expected, "the result reads exactly as the real hosts' address-only result");
  assert.ok(attached, "the snapshot stays attached so the UI can navigate by it");
  assert.equal(attached.presetPosition, snapshot.presetPosition);
  assert.ok(!("changed" in result) && !("readback" in result), "no changed or readback claim");
  assert.notEqual(result.verification, "authoritative_readback");
}

test("the virtual gateway reports recall, bank navigation and reload as an observed address", async () => {
  const gateway = await openVirtual();
  const start = await gateway.currentSnapshot();

  const paged = await gateway.navigateBank(1, start.presetName, start.presetPosition, start.presetIdentity!);
  const afterPage = await gateway.currentSnapshot();
  assert.equal(afterPage.presetPosition, start.presetPosition + 8, "the bank change still moves the virtual device");
  assert.equal(paged.snapshot?.presetPosition, afterPage.presetPosition, "the UI-visible snapshot moved with it");
  assertAddressObserved(paged, afterPage, false);

  const recalled = await gateway.recallPreset(
    start.setlistKey, start.presetPosition, afterPage.presetName, afterPage.presetPosition, afterPage.setlistKey, afterPage.presetIdentity!
  );
  const afterRecall = await gateway.currentSnapshot();
  assert.equal(afterRecall.presetPosition, start.presetPosition, "the recall still moves the virtual device");
  assert.equal(recalled.snapshot?.presetPosition, start.presetPosition, "the UI-visible snapshot moved back");
  assertAddressObserved(recalled, afterRecall, false);

  const reloaded = await gateway.reloadPreset(afterRecall.presetName, afterRecall.presetPosition, afterRecall.presetIdentity!);
  const afterReload = await gateway.currentSnapshot();
  assert.equal(afterReload.dirty, false);
  assertAddressObserved(reloaded, afterReload, true);
});

test("the verification-claim rule catches the old recall timeout wording", () => {
  assert.equal(claimsVerification("Preset recalled and verified; the QC pushed no confirming position and preset within 2500 ms, so the effect is not verified"), true);
  assert.equal(claimsVerification("Preset recall requested; the frame was sent, but the host observed no position update for /user/live position 9 within 2500 ms, so the effect is not verified"), false);
});

test("other virtual writes keep their verified results", async () => {
  // Control: only the three preset-address tools changed shape.
  const gateway = await openVirtual();
  const general = await gateway.generalSettings();
  const original = general.looperStompAssignments as number[];
  const swapped = [...original];
  [swapped[0], swapped[1]] = [swapped[1]!, swapped[0]!];
  const assigned = await gateway.setLooperStompAssignments(swapped, original);
  assert.equal(assigned.verified, true);
  assert.equal(assigned.verification, "authoritative_readback");
  assert.equal(assigned.observation, undefined);
  assert.deepEqual((await gateway.generalSettings()).looperStompAssignments, swapped);
});
