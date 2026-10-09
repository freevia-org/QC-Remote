package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;

public class RelayUnobservedDirtyRefusalTest {
    @Test public void anUnobservedDirtyFlagIsRetryable() {
        // The dirty flag arrives by itself, so the refusal is retryable.
        assertTrue(new RelayException("UNSAVED_STATE_NOT_YET_OBSERVED", "not reported yet").retryable());
    }
}
