package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;

public class RelayProtocolTest {
    @Test public void policyIsGeneratedFromAllIntentLevelMcpActions() {
        assertFalse(RelayProtocol.isAllowed("device.snapshot"));
        assertTrue(RelayProtocol.isAllowed("device.currentPreset"));
        assertTrue(RelayProtocol.isAllowed("device.selectScene"));
        assertTrue(RelayProtocol.isAllowed("device.setParameter"));
        assertTrue(RelayProtocol.isAllowed("device.setMasterVolume"));
        assertTrue(RelayProtocol.isAllowed("device.moveBlock"));
        assertTrue(RelayProtocol.isAllowed("device.addBlock"));
        assertTrue(RelayProtocol.isAllowed("device.removeBlock"));
        assertTrue(RelayProtocol.isAllowed("device.setBlockFootswitch"));
        assertTrue(RelayProtocol.isAllowed("device.setChainInput"));
        assertTrue(RelayProtocol.isAllowed("device.setChainOutput"));
        assertTrue(RelayProtocol.isAllowed("device.setChainSplit"));
        assertTrue(RelayProtocol.isAllowed("device.listModels"));
        assertTrue(RelayProtocol.isAllowed("device.listPresets"));
        assertTrue(RelayProtocol.isAllowed("device.savePresetAs"));
        assertFalse(RelayProtocol.isAllowed("raw.hid.write"));
        assertTrue(RelayProtocol.requiresConfirmation("device.copyPreset"));
        assertTrue(RelayProtocol.requiresConfirmation("device.savePresetAs"));
        assertTrue(RelayProtocol.requiresConfirmation("device.reloadPreset"));
        assertTrue(RelayProtocol.isReadOnly("device.currentPreset"));
        assertTrue(RelayProtocol.isReadOnly("device.masterVolume"));
        assertFalse(RelayProtocol.isReadOnly("device.selectScene"));
        assertFalse(RelayProtocol.isReadOnly("device.setParameter"));
        assertTrue(GeneratedRemoteActions.isPerformance("device.setTempo"));
        assertTrue(GeneratedRemoteActions.isPerformance("device.pressFootswitch"));
        assertFalse(GeneratedRemoteActions.isPerformance("device.selectScene"));
        assertTrue(GeneratedRemoteActions.isModify("device.selectScene"));
        assertTrue(GeneratedRemoteActions.isModify("device.setParameter"));
        assertFalse(GeneratedRemoteActions.isModify("device.setDeviceName"));
    }

    @Test public void relayStateUsesTypedKnownValuesAndFailsClosedForUnknownPersistedText() {
        assertEquals("stopped", RelayState.STOPPED.wireValue());
        assertEquals("connecting", RelayState.CONNECTING.wireValue());
        assertEquals("connected", RelayState.CONNECTED.wireValue());
        assertEquals("reconnecting", RelayState.RECONNECTING.wireValue());
        assertEquals("pairing_required", RelayState.PAIRING_REQUIRED.wireValue());
        assertEquals("invalid_endpoint", RelayState.INVALID_ENDPOINT.wireValue());
        assertEquals(RelayState.PAIRING_REQUIRED, RelayState.parse("pairing_required"));
        assertEquals(RelayState.STOPPED, RelayState.parse("future-state"));
        assertTrue(RelayState.CONNECTED.ownsActiveSocket());
        assertTrue(!RelayState.INVALID_ENDPOINT.ownsActiveSocket());
    }

    @Test public void endpointRequiresPlainHttpsOriginWithoutUserInfoOrQuery() {
        assertEquals("https://qcrc.freevia.org", QcRelayPlugin.normalizedEndpoint(" https://qcrc.freevia.org/ "));
        assertNull(QcRelayPlugin.normalizedEndpoint("http://qcrc.freevia.org"));
        assertNull(QcRelayPlugin.normalizedEndpoint("https://credential@localhost"));
        assertNull(QcRelayPlugin.normalizedEndpoint("https://qcrc.freevia.org?token=secret"));
    }

    @Test public void generatedRuntimeStatusMetadataDescribesTheAndroidNativeGateway() {
        assertTrue(GeneratedGatewayMethods.API_VERSION > 0);
        assertTrue(java.util.Arrays.asList(GeneratedGatewayMethods.CAPABILITIES).contains("nativeGateway"));
        assertTrue(java.util.Arrays.asList(GeneratedGatewayMethods.CAPABILITIES).contains("androidUsbRelay"));
        assertFalse(java.util.Arrays.asList(GeneratedGatewayMethods.CAPABILITIES).contains("nativeBroker"));
    }

    @Test public void relayFrameLimitsCountUtf8BytesRatherThanUtf16Characters() {
        assertEquals(3, QcRelayService.utf8Length("€"));
        assertEquals(4, QcRelayService.utf8Length("\uD83D\uDE00"));
    }

    @Test public void relayRequestDeadlinesAreValidatedAndBounded() throws Exception {
        assertEquals(GeneratedRelayProfile.MAXIMUM_INVOKE_DEADLINE_MS,
            QcRelayService.requestDeadlineMs(new org.json.JSONObject()));
        assertEquals(25_000L, QcRelayService.requestDeadlineMs(
            new org.json.JSONObject().put("deadlineMs", 25_000)));
        assertThrows(IllegalArgumentException.class, () -> QcRelayService.requestDeadlineMs(
            new org.json.JSONObject().put("deadlineMs", -1)));
        assertThrows(IllegalArgumentException.class, () -> QcRelayService.requestDeadlineMs(
            new org.json.JSONObject().put("deadlineMs", GeneratedRelayProfile.MAXIMUM_INVOKE_DEADLINE_MS + 1)));
        assertThrows(IllegalArgumentException.class, () -> QcRelayService.requestDeadlineMs(
            new org.json.JSONObject().put("deadlineMs", "25000")));
    }

    @Test public void unknownDeviceOutcomeIsExplicitlyNonRetryable() throws Exception {
        org.json.JSONObject error = RelayProtocol.error("request", "DEVICE_OUTCOME_UNKNOWN",
            "The device outcome is unknown.", false);
        assertFalse(error.getJSONObject("error").getBoolean("retryable"));
    }

    @Test public void relayInvocationAdmissionStaysWithinItsConcurrentLimit() {
        QcRelayInvocationAdmission admission = new QcRelayInvocationAdmission(2);
        assertTrue(admission.tryAcquire());
        assertTrue(admission.tryAcquire());
        assertFalse(admission.tryAcquire());
        assertEquals(2, admission.activeCount());
        admission.release();
        assertTrue(admission.tryAcquire());
        admission.release();
        admission.release();
        admission.release();
        assertEquals(0, admission.activeCount());
    }

    @Test public void generatedGatewayValidationRejectsCoercionAndArgumentDrift() throws Exception {
        GeneratedGatewayMethods.validateParams("device.setTempo", new org.json.JSONObject()
            .put("bpm", 120).put("expectedTempo", 120).put("expectedPresetName", "Clean")
            .put("expectedPresetIdentity", "native-identity"));
        assertThrows(IllegalArgumentException.class, () -> GeneratedGatewayMethods.validateParams(
            "device.setTempo", new org.json.JSONObject()
                .put("bpm", true).put("expectedTempo", 120).put("expectedPresetName", "Clean")
                .put("expectedPresetIdentity", "native-identity")));
        assertThrows(IllegalArgumentException.class, () -> GeneratedGatewayMethods.validateParams(
            "device.snapshot", new org.json.JSONObject().put("ignored", 1)));
        assertThrows(IllegalArgumentException.class, () -> GeneratedGatewayMethods.validateParams(
            "device.setTempo", new org.json.JSONObject().put("expectedTempo", 120).put("expectedPresetName", "Clean")
                .put("expectedPresetIdentity", "native-identity")));
        assertThrows(IllegalArgumentException.class, () -> GeneratedGatewayMethods.validateParams(
            "device.setTempo", new org.json.JSONObject().put("bpm", 120).put("expectedTempo", 120)
                .put("expectedPresetName", "Clean")));
    }
}
