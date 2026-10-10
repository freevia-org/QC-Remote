package org.freevia.qcremote;
import static org.junit.Assert.*;
import org.junit.Test;
/** Storage only. Native operation admission is tested separately, not reimplemented here. */
public class QcSettingsEchoEvidenceTest {
    private static String ticket(int sequence) { return "{\"connection\":1,\"epoch\":{\"sessionId\":\"session\",\"revision\":0},\"sequence\":"+sequence+"}"; }
    @Test public void noOperationCannotSelectRawCandidate() {
        QcSettingsEchoEvidence evidence = new QcSettingsEchoEvidence();
        QcSettingsEchoEvidence.Fence scope = new QcSettingsEchoEvidence.Fence();
        evidence.observe(9, ticket(1), new byte[]{1});
        assertNull(evidence.candidate(9, scope)); scope.operation="native-operation";
        assertEquals(ticket(1), evidence.candidate(9, scope).observationTicket);
        assertNull(evidence.candidate(3, scope));
    }
    @Test public void rawSparseEchoDoesNotMergeRetainedFieldsAndCopiesInboundBytes() {
        QcSettingsEchoEvidence evidence = new QcSettingsEchoEvidence();
        QcSettingsEchoEvidence.Fence scope = new QcSettingsEchoEvidence.Fence(); scope.operation="native-operation";
        evidence.observe(9, ticket(1), new byte[]{1,2,3});
        byte[] sparse = {4}; evidence.observe(9, ticket(3), sparse); sparse[0] = 9;
        assertArrayEquals(new byte[]{4}, evidence.candidate(9, scope).payload);
        evidence.observe(9, ticket(2), new byte[]{8});
        assertArrayEquals(new byte[]{4}, evidence.candidate(9, scope).payload);
    }
}
