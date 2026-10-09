package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertThrows;

import org.json.JSONObject;
import org.junit.Test;

public class QcGatewayPreflightTest {
    @Test public void parameterWritesRequireTheObservedModelBeforeNativeDispatch() throws Exception {
        for (String method : new String[] { "device.setParameter", "device.previewParameter" }) {
            JSONObject params = new JSONObject().put("row", 0).put("column", 1)
                .put("parameterIndex", 2).put("value", 0.6).put("expectedValue", 0.4)
                .put("expectedScene", 0).put("expectedPresetName", "Test")
                .put("expectedPresetIdentity", "observed-native-A");
            assertThrows(IllegalArgumentException.class, () ->
                GeneratedGatewayMethods.validateParams(method, params));
            params.put("expectedModelId", JSONObject.NULL);
            assertThrows(IllegalArgumentException.class, () ->
                GeneratedGatewayMethods.validateParams(method, params));
            params.put("expectedModelId", "41");
            assertThrows(IllegalArgumentException.class, () ->
                GeneratedGatewayMethods.validateParams(method, params));
            params.put("expectedModelId", 41);
            GeneratedGatewayMethods.validateParams(method, params);
            assertEquals(41, params.getInt("expectedModelId"));
        }
    }

    @Test public void sceneColorRequiresTheObservedColorBeforeNativeDispatch() throws Exception {
        JSONObject params = new JSONObject().put("scene", 2).put("color", 0xffff02c2L)
            .put("expectedPresetName", "Test").put("expectedPresetIdentity", "observed-native-A");
        assertThrows(IllegalArgumentException.class, () ->
            GeneratedGatewayMethods.validateParams("device.setSceneColor", params));
        params.put("expectedColor", JSONObject.NULL);
        assertThrows(IllegalArgumentException.class, () ->
            GeneratedGatewayMethods.validateParams("device.setSceneColor", params));
        params.put("expectedColor", 12);
        assertThrows(IllegalArgumentException.class, () ->
            GeneratedGatewayMethods.validateParams("device.setSceneColor", params));
        params.put("expectedColor", "#AbCdEf");
        GeneratedGatewayMethods.validateParams("device.setSceneColor", params);
        assertEquals("#AbCdEf", params.getString("expectedColor"));
    }

    @Test public void scenePreflightUsesTheDecodedSnapshot() throws Exception {
        JSONObject decodedSnapshot = new JSONObject()
            .put("presetName", "Numb")
            .put("activeScene", 2);

        JSONObject observation = QcGatewayPreflight.authoritativeObservation(
            "decodedSnapshot", () -> decodedSnapshot);

        assertSame(decodedSnapshot, observation);
        assertEquals("Numb", observation.getString("presetName"));
        assertEquals(2, observation.getInt("activeScene"));
    }

    @Test public void aCorrelatedReplySourceIsNoLongerAccepted() {
        assertThrows(IllegalStateException.class, () ->
            QcGatewayPreflight.authoritativeObservation(
                "correlatedReply", () -> { throw new AssertionError("snapshot must not be read"); }));
    }

    @Test public void missingSourceFailsClosed() {
        assertThrows(IllegalStateException.class, () ->
            QcGatewayPreflight.authoritativeObservation(
                null, JSONObject::new));
    }
}
