package org.freevia.qcremote;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Supplier;
import org.json.JSONObject;

/** One lifecycle for every asynchronous native operation awaiting a QC event. */
final class QcPendingOperations {
    static final class Entry<T> {
        final long id;
        final T operation;
        final CompletableFuture<JSONObject> result;

        Entry(long id, T operation, CompletableFuture<JSONObject> result) {
            this.id = id;
            this.operation = operation;
            this.result = result;
        }
    }

    private final AtomicLong ids = new AtomicLong(1);
    private final ConcurrentHashMap<Long, Entry<?>> entries = new ConcurrentHashMap<>();

    <T> Entry<T> register(T operation, CompletableFuture<JSONObject> result) {
        Entry<T> entry = new Entry<>(ids.getAndIncrement(), operation, result);
        entries.put(entry.id, entry);
        return entry;
    }

    boolean remove(Entry<?> entry) { return entries.remove(entry.id, entry); }

    boolean isEmpty() { return entries.isEmpty(); }

    <T> List<Entry<T>> entries(Class<T> type) {
        List<Entry<T>> matching = new ArrayList<>();
        for (Entry<?> entry : entries.values()) {
            if (type.isInstance(entry.operation)) {
                @SuppressWarnings("unchecked") Entry<T> typed = (Entry<T>) entry;
                matching.add(typed);
            }
        }
        return matching;
    }

    void timeout(
        Entry<?> entry, long delayMs, ScheduledExecutorService scheduler,
        Supplier<? extends Throwable> error
    ) {
        scheduler.schedule(() -> {
            if (remove(entry)) entry.result.completeExceptionally(error.get());
        }, Math.max(0, delayMs), TimeUnit.MILLISECONDS);
    }

    void failAll(Supplier<? extends Throwable> error) {
        failAllExcept(null, error);
    }

    void failAllExcept(Entry<?> preserved, Supplier<? extends Throwable> error) {
        for (Entry<?> entry : entries.values()) {
            if (entry == preserved) continue;
            if (remove(entry)) entry.result.completeExceptionally(error.get());
        }
    }
}
