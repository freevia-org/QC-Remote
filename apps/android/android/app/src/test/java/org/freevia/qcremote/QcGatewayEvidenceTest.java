package org.freevia.qcremote;

import com.getcapacitor.JSObject;
import java.util.List;
import java.util.Set;
import org.junit.Test;
import static org.junit.Assert.*;

/** Exercises the production relevant-kind accumulator; native admission is tested at the plugin/owner boundary, with no USB/native calls. */
public class QcGatewayEvidenceTest {
    static QcObservationFence ticket(long sequence) { return ticket(1, "session", 0, sequence); }
    static QcObservationFence ticket(long connection, String session, long revision, long sequence) {
        return new QcObservationFence("{\"connection\":" + connection + ",\"epoch\":{\"sessionId\":\"" + session
            + "\",\"revision\":" + revision + "},\"sequence\":" + sequence + "}");
    }
    private static PendingGatewayTransaction pending(String kind) {
        QcNativeStateDecoder.PlannedGatewayWrite plan = new QcNativeStateDecoder.PlannedGatewayWrite(
            "{}", "fixture", "fixture", "{}", Set.of(kind), false, false, 0, 0,
            false, 0, 100, null, null, null, null, List.of());
        return new PendingGatewayTransaction(plan, 1, 0);
    }
    private static List<JSObject> states(String kind) {
        JSObject state = new JSObject(); state.put("kind", kind); return List.of(state);
    }
    @Test public void admittedTempoAndUnrelatedKindsCannotAdvanceEvidence() {
        PendingGatewayTransaction operation = pending("scene");
        operation.observeAdmitted(ticket(11), List.of());
        operation.observeAdmitted(ticket(12), states("tempo"));
        assertEquals(1, operation.observationSequence());
        operation.observeAdmitted(ticket(13), states("scene"));
        assertEquals(13, operation.observationSequence());
    }
    @Test public void admittedWritesKeepIndependentRelevantTokens() {
        PendingGatewayTransaction scene = pending("scene"), master = pending("master");
        scene.observeAdmitted(ticket(11), states("master")); master.observeAdmitted(ticket(11), states("master"));
        assertEquals(1, scene.observationSequence()); assertEquals(11, master.observationSequence());
        scene.observeAdmitted(ticket(12), states("scene")); master.observeAdmitted(ticket(12), states("scene"));
        assertEquals(12, scene.observationSequence()); assertEquals(11, master.observationSequence());
    }
}
