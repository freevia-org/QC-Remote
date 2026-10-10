package org.freevia.qcremote;

import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import org.json.JSONArray;
import org.json.JSONObject;

/** Caller holds the publication lock while updating the journal and its tail. */
final class QcStateEventJournal<T extends JSONObject> {
    private static final int FRAME_BUDGET = QcDomain.STATE_EVENT_PAGE_MAXIMUM_BYTES - 1024;
    private static final class Frame<T> {
        final T value;
        final int bytes;
        Frame(T value, int bytes) { this.value = value; this.bytes = bytes; }
    }
    private final ArrayDeque<Frame<T>> frames = new ArrayDeque<>();
    private int bytes;

    boolean add(T frame) {
        String json = frame.toString();
        if (json == null || json.length() >= FRAME_BUDGET) return false;
        int size = json.getBytes(StandardCharsets.UTF_8).length + 1;
        if (size > FRAME_BUDGET) return false;
        frames.addLast(new Frame<>(frame, size));
        bytes += size;
        while (frames.size() > QcDomain.STATE_EVENT_MAXIMUM_LIMIT || bytes > QcDomain.STATE_EVENT_HISTORY_MAXIMUM_BYTES) {
            bytes -= frames.removeFirst().bytes;
        }
        return true;
    }

    JSONArray page(long after, int limit) {
        JSONArray page = new JSONArray();
        int size = 0;
        for (Frame<T> frame : frames) {
            if (frame.value.optLong("sequence", 0) <= after) continue;
            if (page.length() >= Math.min(limit, QcDomain.STATE_EVENT_MAXIMUM_LIMIT) || size + frame.bytes > FRAME_BUDGET) break;
            page.put(frame.value);
            size += frame.bytes;
        }
        return page;
    }

    void clear() { frames.clear(); bytes = 0; }
}
