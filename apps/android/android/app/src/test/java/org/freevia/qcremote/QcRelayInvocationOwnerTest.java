package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;

public class QcRelayInvocationOwnerTest {
    @Test public void stopExpiresQueuedWorkAndRetainsTrackingUntilItRetires() {
        QcRelayInvocationOwner<Object> owner=new QcRelayInvocationOwner<>();
        Object invocation=new Object();
        QcRelayDeadlineState state=new QcRelayDeadlineState();
        owner.track(invocation,state::expire);
        owner.expireAll();
        assertFalse(state.beginMutation());
        assertFalse(state.beginRead());
        assertEquals(1,owner.size());
        owner.retire(invocation);
        assertEquals(0,owner.size());
    }
    @Test public void stopPreservesUnknownMutationOutcomeAndDoesNotOwnTheNextService() {
        QcRelayInvocationOwner<Object> stopped=new QcRelayInvocationOwner<>();
        QcRelayDeadlineState sent=new QcRelayDeadlineState();
        assertTrue(sent.beginMutation());
        int[] expiredFrom={-1};
        stopped.track(new Object(),()->expiredFrom[0]=sent.expire());
        QcRelayInvocationOwner<Object> next=new QcRelayInvocationOwner<>();
        QcRelayDeadlineState queuedNext=new QcRelayDeadlineState();
        next.track(new Object(),queuedNext::expire);
        stopped.expireAll();
        assertEquals(QcRelayDeadlineState.MUTATION_ADMITTED,expiredFrom[0]);
        assertTrue(sent.isExpired());
        assertTrue(queuedNext.beginMutation());
    }
    @Test public void synchronousRetirementDuringStopDoesNotSkipOtherExpirations() {
        QcRelayInvocationOwner<Object> owner=new QcRelayInvocationOwner<>();
        Object first=new Object(),second=new Object();
        boolean[] expired={false,false};
        owner.track(first,()->{expired[0]=true;owner.retire(first);});
        owner.track(second,()->{expired[1]=true;owner.retire(second);});
        owner.expireAll();
        assertTrue(expired[0]);assertTrue(expired[1]);assertEquals(0,owner.size());
    }
}
