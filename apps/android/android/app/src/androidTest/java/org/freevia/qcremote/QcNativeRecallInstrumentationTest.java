package org.freevia.qcremote;

import com.getcapacitor.JSObject;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import junit.framework.TestCase;
import org.json.JSONObject;

/** Real packaged JNI, synthetic ingress only. Never opens USB or starts the app/service. */
public final class QcNativeRecallInstrumentationTest extends TestCase {
    private static final String METHOD = "device.recallPreset";
    private static final String FOLDER = "/user/live";
    private interface Checked { void run() throws Exception; }


    private static void rejected(Checked call) throws Exception {
        try { call.run(); }
        catch (IllegalStateException expected) {
            assertNotNull(expected.getMessage());
            assertFalse(expected.getMessage().isEmpty());
            return;
        }
        fail("Expected the real JNI owner/closed-handle refusal");
    }

    private static final class Fixture implements AutoCloseable {
        final QcNativeStateDecoder decoder = new QcNativeStateDecoder();
        long connection;
        long sequence;
        // One clock per owner. Never return to wall time after injecting a deadline.
        private long clock = System.nanoTime() / 1_000_000L;
        long tick() { return ++clock; }
        long at(long value) {
            if (value < clock) throw new IllegalArgumentException("Fixture clock regressed");
            clock = value;
            return clock;
        }
        Fixture() throws Exception {
            try {
                connection = decoder.sessionOpened(tick());
                assertEquals(QcNativeStateDecoder.HandshakeDecision.SEND,
                    decoder.handshakeAttempt(tick(), "jni-recall-fixture").kind);
                // Active SessionValidating is sufficient for this owner API; not Connected.
                assertFalse(decoder.sessionConnected());
                assertTrue(decoder.captureReportEpoch(connection) > 0);
                ingest(2, position(8));
                ingest(15, preset("JNI A"));
                ingest(34, new byte[] {8, 1}); // UPDATE, dirty=false (proto3 default).
                JSONObject snapshot = decoder.snapshot();
                assertNotNull(decoder.currentPresetObservation().getJSONObject("snapshot"));
                assertEquals("JNI A", snapshot.getString("presetName"));
                assertEquals(FOLDER, snapshot.getString("setlistKey"));
                assertEquals(8, snapshot.getInt("presetPosition"));
                assertFalse(snapshot.getBoolean("dirty"));
            } catch (Exception | Error failure) {
                decoder.close();
                throw failure;
            }
        }
        void ingest(int type, byte[] payload) {
            long epoch = decoder.captureReportEpoch(connection);
            QcNativeStateDecoder.DecodedFrame complete = null;
            for (byte[] report : decoder.encodeReports(
                    new QcNativeStateDecoder.EncodedMessage(type, payload), true, null, tick())) {
                report[0] = (byte) QcNativeStateDecoder.IN_REPORT_ID;
                QcNativeStateDecoder.DecodedFrame frame = decoder.pushReport(connection, epoch, report, tick());
                if (frame != null) {
                    assertNull("One synthetic message must yield exactly one completed frame", complete);
                    complete = frame;
                }
            }
            assertNotNull("The production assembler must accept the synthetic frame", complete);
            assertEquals(type, complete.messageType);
            assertTrue(decoder.observationCurrent(complete.observationTicket));
            assertTrue(complete.observationFence.sequence > sequence);
            sequence = complete.observationFence.sequence;
        }
        QcNativeStateDecoder.PlannedGatewayWrite plan() throws Exception {
            JSObject args = new JSObject();
            args.put("setlistKey", FOLDER);
            args.put("position", 9);
            args.put("expectedSetlistKey", FOLDER);
            args.put("expectedPresetName", "JNI A");
            args.put("expectedPosition", 8);
            return decoder.gatewayPlan(METHOD, args);
        }
        JSONObject advance(long id, long time, QcNativeStateDecoder.PlannedGatewayWrite plan) throws Exception {
            return decoder.gatewayVerificationAdvance(id, sequence, at(time), METHOD, plan.verificationJson);
        }
        JSONObject assemble(JSONObject action, QcNativeStateDecoder.PlannedGatewayWrite plan) throws Exception {
            return QcGatewayCompletion.assemble(action, METHOD, plan.detail,
                plan.confirmationTimeoutMs, 0, 0, sequence, decoder::snapshot);
        }
        @Override public void close() { decoder.close(); }
    }

    public void testTimeoutWithoutFreshObservation() throws Exception {
        assertPartialTimeout(0);
    }

    public void testTimeoutWithOnlyFreshPosition() throws Exception {
        assertPartialTimeout(2);
    }

    public void testTimeoutWithOnlyFreshBody() throws Exception {
        assertPartialTimeout(15);
    }

    private static void assertPartialTimeout(int freshType) throws Exception {
        // Each entrypoint owns a separate native decoder/registry/session.
        try (Fixture f = new Fixture()) {
            QcNativeStateDecoder.PlannedGatewayWrite plan = f.plan();
            long started = f.tick();
            long beforeSequence = f.sequence;
            long id = f.decoder.gatewayVerificationStarted(plan, beforeSequence, started);
            assertTrue(id > 0);
            if (freshType == 2) {
                f.ingest(2, position(9));
                assertEquals(9, f.decoder.snapshot().getInt("presetPosition"));
            } else if (freshType == 15) {
                f.ingest(15, preset("JNI B"));
                assertEquals("JNI B", f.decoder.snapshot().getString("presetName"));
                assertEquals(8, f.decoder.snapshot().getInt("presetPosition"));
            }
            if (freshType != 0) assertTrue(f.sequence > beforeSequence);
            else assertEquals(beforeSequence, f.sequence);
            long waitingAt = f.tick();
            assertTrue("Fixture ingress must remain inside the native verification budget",
                waitingAt < started + plan.confirmationTimeoutMs);
            JSONObject waiting = f.advance(id, waitingAt, plan);
            assertEquals("wait", waiting.getString("kind"));
            assertTrue(waiting.getLong("delayMs") > 0);
            assertFalse(waiting.has("outcome"));
            JSONObject action = f.advance(id, started + plan.confirmationTimeoutMs + 1, plan);
            assertEquals("timedOut", action.getString("kind"));
            assertEquals(1, action.length());
            JSONObject result = f.assemble(action, plan);
            assertTrue(result.getBoolean("accepted"));
            assertFalse(result.getBoolean("verified"));
            assertEquals("accepted_unverified", result.getString("verification"));
            assertFalse(result.has("changed"));
            assertFalse(result.has("readback"));
            assertFalse("Partial observations must not leak as completion evidence", result.has("observation"));
            assertFalse(result.has("outcome"));
            assertFalse(result.has("pluginBlockReadback"));
            assertTrue(result.getString("detail").contains("address"));
            // advance() raised this fixture's clock to the deadline; tick stays beyond it.
            rejected(() -> f.advance(id, f.tick(), plan));
        }
    }

    public void testQualifiedVerifiedTraversesRealJniAndTerminalAssembly() throws Exception {
        try (Fixture f = new Fixture()) {
            QcNativeStateDecoder.PlannedGatewayWrite plan = f.plan();
            long started = f.tick();
            long id = f.decoder.gatewayVerificationStarted(plan, f.sequence, started);
            f.ingest(2, position(9));
            f.ingest(15, preset("JNI B"));
            long observedAt = f.tick();
            assertTrue(observedAt < started + plan.confirmationTimeoutMs);
            JSONObject action = f.advance(id, observedAt, plan);
            assertEquals("verified", action.getString("kind"));
            JSONObject outcome = action.getJSONObject("outcome");
            assertFalse(outcome.getBoolean("verified"));
            assertEquals("accepted_unverified", outcome.getString("verification"));
            assertFalse(outcome.has("changed"));
            assertFalse(outcome.has("readback"));
            assertEquals("preset_address_observed", outcome.getJSONObject("observation").getString("kind"));
            assertEquals("unbound", outcome.getJSONObject("observation").getString("body"));
            assertFalse(outcome.getJSONObject("observation").getBoolean("causallyCorrelated"));
            JSONObject result = f.assemble(action, plan);
            assertFalse(result.getBoolean("verified"));
            assertEquals("accepted_unverified", result.getString("verification"));
            assertEquals(outcome.getJSONObject("observation").toString(),
                result.getJSONObject("observation").toString());
            assertFalse(result.has("changed"));
            assertFalse(result.has("readback"));
            rejected(() -> f.advance(id, f.tick(), plan));
        }
    }

    public void testInvalidCancelledAndReplacedSessionIdsThrowAcrossJni() throws Exception {
        try (Fixture f = new Fixture()) {
            QcNativeStateDecoder.PlannedGatewayWrite plan = f.plan();
            rejected(() -> f.advance(0, f.tick(), plan));
            long cancelled = f.decoder.gatewayVerificationStarted(plan, f.sequence, f.tick());
            f.decoder.gatewayVerificationCancelled(cancelled);
            rejected(() -> f.advance(cancelled, f.tick(), plan));
            long replaced = f.decoder.gatewayVerificationStarted(plan, f.sequence, f.tick());
            f.decoder.sessionOpened(f.tick());
            assertEquals(QcNativeStateDecoder.HandshakeDecision.SEND,
                f.decoder.handshakeAttempt(f.tick(), "jni-recall-replacement").kind);
            rejected(() -> f.advance(replaced, f.tick(), plan));
        }
    }

    public void testFinallyClosesAfterExceptionalExit() throws Exception {
        QcNativeStateDecoder decoder = null;
        try {
            try (Fixture f = new Fixture()) {
                decoder = f.decoder;
                throw new IllegalArgumentException("fixture-owned-exit");
            }
        } catch (IllegalArgumentException expected) {
            assertEquals("fixture-owned-exit", expected.getMessage());
        }
        assertNotNull(decoder);
        final QcNativeStateDecoder closed = decoder;
        rejected(closed::snapshot);
        closed.close(); // Idempotence, no native handle reused.
    }

    // Input protobuf fixtures only. Output JSON is always the actual JNI serializer.
    // ProductionAutomation.proto: UPDATE=1, position fields1/3/4, recall fields1/3.
    // Preset.proto: BinaryPreset.name=2. No production state setter is exported.
    private static byte[] position(int value) {
        return join(new byte[] {8, 1}, field(3, FOLDER.getBytes(StandardCharsets.UTF_8)),
            new byte[] {32, (byte) value});
    }
    private static byte[] preset(String name) {
        return join(new byte[] {8, 1}, field(3, field(2, name.getBytes(StandardCharsets.UTF_8))));
    }
    private static byte[] field(int number, byte[] value) {
        if (value.length >= 128) throw new IllegalArgumentException("Fixture requires a one-byte length");
        return join(new byte[] {(byte) ((number << 3) | 2), (byte) value.length}, value);
    }
    private static byte[] join(byte[]... chunks) {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        for (byte[] chunk : chunks) bytes.write(chunk, 0, chunk.length);
        return bytes.toByteArray();
    }
}
