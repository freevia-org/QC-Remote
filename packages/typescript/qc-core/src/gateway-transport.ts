import { QC_GRID_COLUMNS, requireModelId, requirePresetIdentity } from "@qc-remote/client";
import type { GatewayTransport, PresetSnapshot } from "@qc-remote/client";
import { adjacentPresetPosition, presetBoundaryDetail } from "./state.ts";
import type { QcDeviceTransport } from "./transport.ts";

// Catalog id for the built-in Mixer; verified in skills/qc-device-knowledge/references/data/qc-model-lineage.json.
const QC_MIXER_MODEL_ID = 11000;

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
    currentSnapshot() {
      return gateway.currentSnapshot();
    },
    selectScene(scene, snapshot) {
      const state = expected(snapshot);
      return gateway.selectScene(scene, state.presetName, requirePresetIdentity(state));
    },
    selectModeSlot(slot, snapshot) {
      const state = expected(snapshot);
      return gateway.selectModeSlot(slot, state.presetName, requirePresetIdentity(state));
    },
    movePreset(delta, snapshot) {
      const state = expected(snapshot);
      const position = adjacentPresetPosition(state.presetPosition, delta);
      // Recalling the loaded preset again is not a step and may reload it.
      if (position === state.presetPosition) return Promise.resolve({ detail: presetBoundaryDetail(delta) });
      // An empty expected name deliberately avoids comparing the new preset's
      // name while retaining the position guard that serializes rapid taps.
      return gateway.recallPreset(state.setlistKey, position, "", state.presetPosition, state.setlistKey, requirePresetIdentity(state));
    },
    pressFootswitch(index, snapshot) {
      const state = expected(snapshot);
      return gateway.pressFootswitch(index, state.mode, state.presetName, requirePresetIdentity(state));
    },
    setTempo(bpm, snapshot) {
      const state = expected(snapshot);
      return gateway.setTempo(bpm, state.tempo, state.presetName, requirePresetIdentity(state));
    },
    tapTempo(snapshot) {
      const state = expected(snapshot);
      return gateway.tapTempo(state.mode, state.presetName, requirePresetIdentity(state));
    },
    blockDetails(row, column, snapshot) {
      const state = expected(snapshot);
      return gateway.blockDetails(row, column, state.presetName, requirePresetIdentity(state));
    },
    setParameter(row, column, parameterIndex, value, snapshot) {
      const state = expected(snapshot);
      const route = state.routes.find((route) => route.row === row);
      const modelId = column < QC_GRID_COLUMNS
        ? requireModelId(state.blocks.find((block) => block.row === row && block.column === column))
        : column === QC_GRID_COLUMNS
          ? requireModelId({modelId: route?.splitterModelId})
          : requireModelId({modelId: route?.mixColumn != null && route.mixColumn >= 0 ? QC_MIXER_MODEL_ID : undefined});
      return gateway.blockDetails(row, column, state.presetName, requirePresetIdentity(state)).then((block) => {
        if (block.modelId !== modelId) throw new Error("The parameter's block changed. Refresh and retry.");
        const parameter = block.parameters.find((candidate) => candidate.index === parameterIndex);
        if (!parameter || parameter.normalizedValue === null) throw new Error(`Parameter ${parameterIndex} is not writable.`);
        return gateway.setParameter(row, column, parameterIndex, value, parameter.normalizedValue, state.activeScene, modelId, state.presetName, requirePresetIdentity(state));
      });
    },
    setBypass(row, column, bypassed, snapshot) {
      const state = expected(snapshot);
      const block = state.blocks.find((candidate) => candidate.row === row && candidate.column === column);
      if (!block) return Promise.reject(new Error(`No block exists at row ${row + 1}, column ${column + 1}.`));
      return gateway.toggleBypass(row, column, state.activeScene, block.bypassed ?? false, bypassed, requireModelId(block), state.presetName, requirePresetIdentity(state));
    },
    setTuner(show) {
      return gateway.showTuner(show);
    },
    setGigView(show) {
      return gateway.showGigView(show);
    }
  };
}
