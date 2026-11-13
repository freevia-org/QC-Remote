package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayList;
import java.util.List;
import org.json.JSONObject;
import org.junit.Test;

public final class QcUsbFlightRecorderEvictionTest {
    private static JSONObject entry(String event, int type) throws Exception {
        JSONObject entry = new JSONObject().put("event", event);
        if (type >= 0) entry.put("messageType", type);
        return entry;
    }

    @Test public void trafficIsEvictedBeforeAnyLifecycleEvent() throws Exception {
        List<JSONObject> entries = new ArrayList<>();
        entries.add(entry("usb-attached", -1));
        entries.add(entry("handshake-attempt-1", -1));
        entries.add(entry("inbound", 35));
        entries.add(entry("startup-initializing", -1));
        entries.add(entry("outbound", 10));
        for (int round = 0; round < 1000; round++) {
            entries.add(entry("inbound", 35));
            JSONObject evicted = QcUsbFlightRecorder.evictionCandidate(entries);
            assertNotNull(evicted);
            assertTrue("only traffic is evicted", evicted.optString("event").equals("inbound")
                || evicted.optString("event").equals("outbound"));
            entries.remove(evicted);
        }
        List<String> kept = new ArrayList<>();
        for (JSONObject kept1 : entries) kept.add(kept1.optString("event"));
        assertTrue(kept.contains("usb-attached"));
        assertTrue(kept.contains("handshake-attempt-1"));
        assertTrue(kept.contains("startup-initializing"));
    }

    @Test public void keepalivesAreEvictedBeforeOtherTraffic() throws Exception {
        List<JSONObject> entries = new ArrayList<>();
        entries.add(entry("inbound", 35));
        JSONObject keepalive = entry("outbound", QcUsbProfile.MESSAGE_TYPE_KEEP_ALIVE);
        entries.add(keepalive);
        assertSame(keepalive, QcUsbFlightRecorder.evictionCandidate(entries));
    }

    @Test public void aRecordOfOnlyLifecycleEventsHasNoTrafficToEvict() throws Exception {
        List<JSONObject> entries = new ArrayList<>();
        entries.add(entry("usb-attached", -1));
        assertNull(QcUsbFlightRecorder.evictionCandidate(entries));
    }
}
