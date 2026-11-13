import { requireModelId, requirePresetIdentity } from "@qc-remote/client";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { adjacentPresetPosition, presetBoundaryDetail } from "./state.ts";
import type { QcDeviceTransport } from "./transport.ts";

/**
 * The single UI-to-gateway adapter used by both desktop and Android shells.
 * Native code is responsible only for carrying gateway calls to the device.
 */
export function createQcGatewayTransport(
  gateway: GatewayTransport,
  currentSnapshot: () => PresetSnapshot
): QcDeviceTransport {
  const expected = (snapshot?: PresetSnapshot) => snapshot ?? currentSnapshot();
  return {
    async currentSnapshot() {
      return gateway.currentSnapshot();
    },
    async selectScene(scene, snapshot) {
      const state = expected(snapshot);
      return gateway.selectScene(scene, state.presetName, requirePresetIdentity(state));
    },
    async selectModeSlot(slot, snapshot) {
      const state = expected(snapshot);
      return gateway.selectModeSlot(slot, state.presetName, requirePresetIdentity(state));
    },
    async movePreset(delta, snapshot) {
      const state = expected(snapshot);
      const position = adjacentPresetPosition(state.presetPosition, delta);
      // Recalling the loaded preset again is not a step and may reload it.
      if (position === state.presetPosition) return { detail: presetBoundaryDetail(delta) };
      // Recall guards describe the loaded preset, not the destination. The
      // cloud contract requires its name as well as its address and identity.
      return gateway.recallPreset(state.setlistKey, position, state.presetName, state.presetPosition, state.setlistKey, requirePresetIdentity(state));
    },
    async pressFootswitch(index, snapshot) {
      const state = expected(snapshot);
      return gateway.pressFootswitch(index, state.mode, state.presetName, requirePresetIdentity(state));
    },
    async setTempo(bpm, snapshot) {
      const state = expected(snapshot);
      return gateway.setTempo(bpm, state.tempo, state.presetName, requirePresetIdentity(state));
    },
    async tapTempo(snapshot) {
      const state = expected(snapshot);
      return gateway.tapTempo(state.mode, state.presetName, requirePresetIdentity(state));
    },
    async setBypass(row, column, bypassed, snapshot) {
      const state = expected(snapshot);
      const block = state.blocks.find((candidate) => candidate.row === row && candidate.column === column);
      if (!block) throw new Error(`No block exists at row ${row + 1}, column ${column + 1}.`);
      return gateway.toggleBypass(row, column, state.activeScene, block.bypassed ?? false, bypassed, requireModelId(block), state.presetName, requirePresetIdentity(state));
    },
    async setTuner(show) {
      return gateway.showTuner(show);
    },
    async setGigView(show) {
      return gateway.showGigView(show);
    }
  };
}
