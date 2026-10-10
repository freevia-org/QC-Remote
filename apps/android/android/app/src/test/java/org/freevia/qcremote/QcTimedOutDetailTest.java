package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;

public class QcTimedOutDetailTest {
    /** The shared rule: an unverified detail says "verified" only inside "not verified" or "unverified". */
    private static boolean claimsVerification(String detail) {
        return detail.toLowerCase(java.util.Locale.ROOT)
            .replace("not verified", "").replace("unverified", "").contains("verified");
    }

    @Test public void aRecallTimeoutSaysWhatWasNotObservedAndClaimsNothing() {
        // The native timedOut action carries no evidence, so these texts must be true for every
        // partial observation the shared predicate rejects: (a) newer position, old body;
        // (b) newer body, no newer position; (c) reload: position and body, no clean flag;
        // (d) nothing. They name the required conditions and assert none of them absent.
        String[][] cases = {
            {"device.recallPreset", "Preset recall requested", "naming the requested preset address and a newer full preset publication)"},
            {"device.navigateBank", "Preset bank change requested", "naming the requested preset address and a newer full preset publication)"},
            {"device.reloadPreset", "Active preset address resent", "naming the active preset address, a newer full preset publication and a clean flag)"},
        };
        assertEquals(3, cases.length);
        for (String[] row : cases) {
            String detail = QcTimedOutDetail.of(row[0], row[1], 2500);
            assertTrue(detail, detail.startsWith(row[1] + "; the frame was sent, but within 2500 ms the host did not observe all of the required conditions together (a newer position update "));
            assertTrue(detail, detail.contains(row[2]));
            for (String absence : new String[] {"no position update", "no newer", "no full preset", "no clean flag", "without a clean flag"}) {
                assertFalse(detail, detail.contains(absence));
            }
            assertFalse(detail, claimsVerification(detail));
        }
        assertTrue(claimsVerification(
            "Preset recalled and verified; the QC pushed no confirming state within 2500 ms, so the effect is not verified"));
    }

    @Test public void otherWritesKeepTheirTimeoutText() {
        assertEquals("setTempo sent to the Quad Cortex; the QC pushed no confirming state within 2500 ms, so the effect is not verified",
            QcTimedOutDetail.of("device.setTempo", "setTempo sent to the Quad Cortex", 2500));
    }
}
