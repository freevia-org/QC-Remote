package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.nio.charset.StandardCharsets;
import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.Test;

public class QcStateEventJournalTest {
    private JSONObject frame(long sequence, String text) throws Exception {
        return new JSONObject().put("sequence", sequence).put("states", text);
    }

    @Test public void bytePagesProgressWithoutSkippingRetainedFrames() throws Exception {
        QcStateEventJournal<JSONObject> journal = new QcStateEventJournal<>();
        for (int sequence = 1; sequence <= 3; sequence++) assertTrue(journal.add(frame(sequence, "x".repeat(2_000_000))));
        JSONArray first = journal.page(0, 4096);
        assertEquals(2, first.length());
        assertEquals(1, first.getJSONObject(0).getLong("sequence"));
        assertEquals(2, first.getJSONObject(1).getLong("sequence"));
        assertTrue(first.toString().getBytes(StandardCharsets.UTF_8).length < QcDomain.STATE_EVENT_PAGE_MAXIMUM_BYTES);
        assertEquals(3, journal.page(2, 4096).getJSONObject(0).getLong("sequence"));
        assertEquals(1, journal.page(0, 1).length());
    }

    @Test public void historyCountsUtf8AndEscapingAndClearReclaimsBudget() throws Exception {
        QcStateEventJournal<JSONObject> journal = new QcStateEventJournal<>();
        for (int sequence = 1; sequence <= 4; sequence++) assertTrue(journal.add(frame(sequence, "\"界".repeat(600_000))));
        assertEquals(3, journal.page(0, 4096).getJSONObject(0).getLong("sequence"));
        journal.clear();
        assertEquals(0, journal.page(0, 4096).length());
        assertTrue(journal.add(frame(5, "small")));
        assertEquals(5, journal.page(0, 4096).getJSONObject(0).getLong("sequence"));
    }

    @Test public void oversizedFramesLeaveAGapAndDoNotBlockOrEvictHistory() throws Exception {
        QcStateEventJournal<JSONObject> journal = new QcStateEventJournal<>();
        assertTrue(journal.add(frame(1, "small")));
        assertFalse(journal.add(frame(2, "x".repeat(QcDomain.STATE_EVENT_PAGE_MAXIMUM_BYTES))));
        assertTrue(journal.add(frame(3, "small")));
        JSONArray page = journal.page(0, 4096);
        assertEquals(2, page.length());
        assertEquals(1, page.getJSONObject(0).getLong("sequence"));
        assertEquals(3, page.getJSONObject(1).getLong("sequence"));
    }

    @Test public void countLimitStillEvictsOldestFrames() throws Exception {
        QcStateEventJournal<JSONObject> journal = new QcStateEventJournal<>();
        for (int sequence = 1; sequence <= 4098; sequence++) journal.add(frame(sequence, "small"));
        JSONArray page = journal.page(0, 4096);
        assertEquals(4096, page.length());
        assertEquals(3, page.getJSONObject(0).getLong("sequence"));
    }
}
