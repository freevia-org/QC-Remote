package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONObject;
import org.junit.Test;

public class RelayFrameHandlerTest {
    @Test public void expectedBrokerTargetIsPreservedAndInvalidShapesAreRefused() throws Exception {
        RelayFrameHandler handler = new RelayFrameHandler();
        QcBrokerTarget target = new QcBrokerTarget("demo", "broker-session", 12);
        JSONObject request = new JSONObject().put("type", "invoke").put("id", "target")
            .put("method", "device.setTempo").put("params", new JSONObject().put("bpm", 123))
            .put("expectedBrokerTarget", target.json());
        RelayFrameHandler.Decision decision = handler.inspect(request.toString(), ALLOW_ALL, ALLOW_ALL);
        assertEquals(RelayFrameHandler.Kind.ACCEPT, decision.kind);
        assertTrue(target.same(decision.expectedBrokerTarget));
        request.put("id", "bad-target").put("expectedBrokerTarget", "demo");
        assertEquals("INVALID_ARGUMENT", handler.inspect(request.toString(), ALLOW_ALL, ALLOW_ALL).errorCode);
        request.put("expectedBrokerTarget", target.json());
        assertEquals("REPLAYED_REQUEST", handler.inspect(request.toString(), ALLOW_ALL, ALLOW_ALL).errorCode);
    }
    private static final RelayFrameHandler.MethodPolicy ALLOW_ALL = method -> true;
    private static final RelayFrameHandler.MethodPolicy DENY_ALL = method -> false;

    private static String request(String id, String method, String params, String expected, String deadline) {
        return "{\"type\":\"invoke\",\"id\":" + JSONObject.quote(id)
            + ",\"method\":" + JSONObject.quote(method)
            + (params == null ? "" : ",\"params\":" + params)
            + (expected == null ? "" : ",\"expectedState\":" + expected)
            + (deadline == null ? "" : ",\"deadlineMs\":" + deadline) + "}";
    }

    @Test public void acceptsOnlyWellFormedInvokeAndCarriesTypedArgumentsAndDeadline() {
        RelayFrameHandler handler = new RelayFrameHandler();
        RelayFrameHandler.Decision decision = handler.inspect(
            request("abc", "device.currentPreset", "{}", "{\"presetName\":\"A\"}", "25000"),
            ALLOW_ALL, ALLOW_ALL);
        assertEquals(RelayFrameHandler.Kind.ACCEPT, decision.kind);
        assertEquals("abc", decision.id);
        assertEquals("device.currentPreset", decision.method);
        assertNotNull(decision.params);
        assertEquals("A", decision.expected.optString("presetName"));
        assertEquals(25_000L, decision.deadlineMs);
    }

    @Test public void malformedOversizedWrongTypeAndInvalidDeadlineAreRejectedBeforeDispatch() {
        RelayFrameHandler handler = new RelayFrameHandler();
        RelayFrameHandler.Decision malformed = handler.inspect("not-json", ALLOW_ALL, ALLOW_ALL);
        assertEquals(RelayFrameHandler.Kind.ERROR, malformed.kind);
        assertEquals("INVALID_REQUEST", malformed.errorCode);

        RelayFrameHandler.Decision wrongType = handler.inspect(
            "{\"type\":\"ready\",\"id\":\"x\"}", ALLOW_ALL, ALLOW_ALL);
        assertEquals("INVALID_REQUEST", wrongType.errorCode);

        RelayFrameHandler.Decision oversized = handler.inspect(
            "x".repeat(GeneratedRelayProfile.MAX_REQUEST_FRAME_BYTES + 1), ALLOW_ALL, ALLOW_ALL);
        assertEquals(RelayFrameHandler.Kind.CLOSE, oversized.kind);
        assertEquals(1009, oversized.closeCode);

        RelayFrameHandler.Decision deadline = handler.inspect(
            request("deadline", "device.currentPreset", "{}", null, "0"), ALLOW_ALL, ALLOW_ALL);
        assertEquals("INVALID_REQUEST", deadline.errorCode);
    }

    @Test public void rejectsJsonCoercionsForRequestIdTypeAndMethod() {
        RelayFrameHandler handler = new RelayFrameHandler();
        RelayFrameHandler.Decision numericId = handler.inspect(
            "{\"type\":\"invoke\",\"id\":7,\"method\":\"device.currentPreset\"}",
            ALLOW_ALL, ALLOW_ALL);
        assertEquals("INVALID_REQUEST", numericId.errorCode);

        RelayFrameHandler.Decision numericType = handler.inspect(
            "{\"type\":7,\"id\":\"type-id\",\"method\":\"device.currentPreset\"}",
            ALLOW_ALL, ALLOW_ALL);
        assertEquals("INVALID_REQUEST", numericType.errorCode);

        RelayFrameHandler.Decision numericMethod = handler.inspect(
            "{\"type\":\"invoke\",\"id\":\"method-id\",\"method\":17}",
            ALLOW_ALL, ALLOW_ALL);
        assertEquals("method-id", numericMethod.id);
        assertEquals("INVALID_REQUEST", numericMethod.errorCode);
        RelayFrameHandler.Decision replay = handler.inspect(
            request("method-id", "device.currentPreset", "{}", null, null), ALLOW_ALL, ALLOW_ALL);
        assertEquals("REPLAYED_REQUEST", replay.errorCode);
    }

    @Test public void replayCacheAndAdmissionOrderingAreBehavioral() {
        RelayFrameHandler handler = new RelayFrameHandler();
        String denied = request("same", "device.setTempo", "{}", null, null);
        RelayFrameHandler.Decision first = handler.inspect(denied, ALLOW_ALL, DENY_ALL);
        assertEquals("ACCESS_MODE_RESTRICTED", first.errorCode);
        RelayFrameHandler.Decision replay = handler.inspect(denied, ALLOW_ALL, ALLOW_ALL);
        assertEquals("REPLAYED_REQUEST", replay.errorCode);

        RelayFrameHandler.Decision forbidden = handler.inspect(
            request("forbidden", "device.raw.request", "{}", null, null), DENY_ALL, ALLOW_ALL);
        assertEquals("METHOD_NOT_ALLOWED", forbidden.errorCode);
        RelayFrameHandler.Decision forbiddenReplay = handler.inspect(
            request("forbidden", "device.currentPreset", "{}", null, null), ALLOW_ALL, ALLOW_ALL);
        assertEquals("REPLAYED_REQUEST", forbiddenReplay.errorCode);
    }

    @Test public void invalidShapesConsumeRequestIdButUtf8OversizeDoesNot() {
        RelayFrameHandler handler = new RelayFrameHandler();
        RelayFrameHandler.Decision invalidParams = handler.inspect(
            request("shape", "device.currentPreset", "[]", null, null), ALLOW_ALL, ALLOW_ALL);
        assertEquals("INVALID_ARGUMENT", invalidParams.errorCode);
        assertEquals("REPLAYED_REQUEST", handler.inspect(
            request("shape", "device.currentPreset", "{}", null, null), ALLOW_ALL, ALLOW_ALL).errorCode);

        String oversizedParams = "{\"value\":" + JSONObject.quote(
            "€".repeat(GeneratedRelayProfile.MAX_REQUEST_FRAME_BYTES / 3)) + "}";
        RelayFrameHandler.Decision tooLarge = handler.inspect(
            request("unicode-id", "device.currentPreset", oversizedParams, null, null),
            ALLOW_ALL, ALLOW_ALL);
        assertEquals(RelayFrameHandler.Kind.CLOSE, tooLarge.kind);
        assertEquals(RelayFrameHandler.Kind.ACCEPT, handler.inspect(
            request("unicode-id", "device.currentPreset", "{\"value\":\"ok\"}", null, null),
            ALLOW_ALL, ALLOW_ALL).kind);
    }
}
