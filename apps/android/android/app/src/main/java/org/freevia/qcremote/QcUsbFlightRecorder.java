package org.freevia.qcremote;

import android.content.Context;
import android.os.Process;
import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;
import org.json.JSONArray;
import org.json.JSONObject;

/** A bounded, payload-free USB lifecycle record that survives app restarts. */
final class QcUsbFlightRecorder {
    private static final int MAX_ENTRIES = 256;
    private static final long MAX_FILE_BYTES = 256 * 1024L;
    private static final long CHECKPOINT_INTERVAL_MS = 1_000L;
    private final File path;
    private final Deque<JSONObject> entries = new ArrayDeque<>();
    private final ExecutorService persistenceIo = Executors.newSingleThreadExecutor();
    private final AtomicReference<byte[]> pendingSnapshot = new AtomicReference<>();
    private final AtomicBoolean persistenceScheduled = new AtomicBoolean();
    private final Object fileLock = new Object();
    private long lastPersistedAt;
    private boolean closed;

    QcUsbFlightRecorder(Context context) {
        path = new File(context.getFilesDir(), "qc-usb-flight-recorder.json");
        load();
    }

    synchronized void event(String event) {
        push(entry(event, null, null), true);
    }

    synchronized void activity(String event) {
        push(entry(event, null, null), false);
    }

    synchronized void outbound(int messageType, int reportCount) {
        push(entry("outbound", messageType, reportCount), false);
    }

    synchronized void inbound(int messageType) {
        push(entry("inbound", messageType, null), false);
    }

    synchronized JSObject snapshot() {
        JSObject result = new JSObject();
        result.put("version", 1);
        JSArray copy = new JSArray();
        for (JSONObject entry : entries) copy.put(entry);
        result.put("entries", copy);
        return result;
    }

    synchronized void close() {
        if (closed) return;
        closed = true;
        persistenceIo.shutdown();
        try {
            if (!persistenceIo.awaitTermination(2, TimeUnit.SECONDS)) persistenceIo.shutdownNow();
        } catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            persistenceIo.shutdownNow();
        }
        byte[] snapshot = documentBytes();
        synchronized (fileLock) { persist(snapshot); }
    }

    private JSONObject entry(String event, Integer messageType, Integer reportCount) {
        JSONObject entry = new JSONObject();
        put(entry, "atUnixMs", System.currentTimeMillis());
        put(entry, "processId", Process.myPid());
        put(entry, "event", safeEvent(event));
        if (messageType != null) put(entry, "messageType", messageType);
        if (reportCount != null) put(entry, "reportCount", reportCount);
        return entry;
    }

    private static String safeEvent(String value) {
        if (value == null || !value.matches("[A-Za-z0-9._:-]{1,96}")) return "invalid-event";
        return value;
    }

    private static void put(JSONObject object, String key, Object value) {
        try {
            object.put(key, value);
        } catch (org.json.JSONException ignored) {}
    }

    private void push(JSONObject entry, boolean critical) {
        if (closed) return;
        entries.addLast(entry);
        while (entries.size() > MAX_ENTRIES) {
            JSONObject evicted = null;
            for (JSONObject candidate : entries) {
                if (isRoutineLiveness(candidate)) {
                    evicted = candidate;
                    break;
                }
            }
            if (evicted == null) entries.removeFirst();
            else entries.remove(evicted);
        }
        long now = System.currentTimeMillis();
        if (critical) {
            byte[] snapshot = documentBytes();
            lastPersistedAt = now;
            persistAsync(snapshot);
        } else if (now - lastPersistedAt >= CHECKPOINT_INTERVAL_MS) {
            byte[] snapshot = documentBytes();
            lastPersistedAt = now;
            persistAsync(snapshot);
        }
    }

    private void persistAsync(byte[] snapshot) {
        pendingSnapshot.set(snapshot);
        if (!persistenceScheduled.compareAndSet(false, true)) return;
        try {
            persistenceIo.execute(this::drainPersistQueue);
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            persistenceScheduled.set(false);
            // close() writes a final snapshot after draining the executor.
        }
    }

    private void drainPersistQueue() {
        while (true) {
            byte[] snapshot = pendingSnapshot.getAndSet(null);
            if (snapshot != null) synchronized (fileLock) { persist(snapshot); }
            persistenceScheduled.set(false);
            if (pendingSnapshot.get() == null || !persistenceScheduled.compareAndSet(false, true)) return;
        }
    }

    private boolean isRoutineLiveness(JSONObject entry) {
        String event = entry.optString("event");
        return ("outbound".equals(event) || "inbound".equals(event))
            && entry.optInt("messageType", -1) == QcUsbProfile.MESSAGE_TYPE_KEEP_ALIVE;
    }

    private void load() {
        if (!path.isFile()) return;
        if (path.length() <= 0 || path.length() > MAX_FILE_BYTES) return;
        try (FileInputStream input = new FileInputStream(path)) {
            byte[] bytes = new byte[(int) path.length()];
            int offset = 0;
            while (offset < bytes.length) {
                int count = input.read(bytes, offset, bytes.length - offset);
                if (count < 0) break;
                offset += count;
            }
            JSONObject document = new JSONObject(new String(bytes, 0, offset, StandardCharsets.UTF_8));
            JSONArray stored = document.optJSONArray("entries");
            if (stored == null) return;
            int start = Math.max(0, stored.length() - MAX_ENTRIES);
            for (int index = start; index < stored.length(); index++) {
                JSONObject entry = stored.optJSONObject(index);
                if (entry != null) entries.addLast(sanitize(entry));
            }
        } catch (Exception ignored) {
            entries.clear();
        }
    }

    private JSONObject sanitize(JSONObject source) {
        JSONObject clean = new JSONObject();
        put(clean, "atUnixMs", Math.max(0L, source.optLong("atUnixMs", 0L)));
        put(clean, "processId", Math.max(0, source.optInt("processId", 0)));
        put(clean, "event", safeEvent(source.optString("event", "invalid-event")));
        int messageType = source.optInt("messageType", -1);
        if (messageType >= 0 && messageType <= 65_535) put(clean, "messageType", messageType);
        int reportCount = source.optInt("reportCount", -1);
        if (reportCount >= 0 && reportCount <= 65_535) put(clean, "reportCount", reportCount);
        return clean;
    }

    private byte[] documentBytes() {
        JSONObject document = new JSONObject();
        put(document, "version", 1);
        put(document, "entries", new JSONArray(entries));
        return document.toString().getBytes(StandardCharsets.UTF_8);
    }

    private void persist(byte[] bytes) {
        File temporary = new File(path.getParentFile(), path.getName() + ".tmp");
        try (FileOutputStream output = new FileOutputStream(temporary, false)) {
            output.write(bytes);
            output.flush();
            output.getFD().sync();
            if (!temporary.renameTo(path)) {
                try (FileOutputStream direct = new FileOutputStream(path, false)) {
                    direct.write(bytes);
                    direct.flush();
                    direct.getFD().sync();
                }
                temporary.delete();
            }
        } catch (Exception ignored) {
            temporary.delete();
        }
    }
}
