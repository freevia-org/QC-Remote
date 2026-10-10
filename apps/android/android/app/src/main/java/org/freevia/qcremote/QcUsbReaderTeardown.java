package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/** Orders a replacement USB open after readers from destroyed plugin instances exit. */
final class QcUsbReaderTeardown {
    private static final QcUsbReaderTeardown PROCESS = new QcUsbReaderTeardown();
    private CompletableFuture<Boolean> pending = CompletableFuture.completedFuture(true);

    static QcUsbReaderTeardown process() { return PROCESS; }

    synchronized void trackShutdown(ExecutorService reader) {
        reader.shutdownNow();
        CompletableFuture<Boolean> stopped = new CompletableFuture<>();
        Thread waiter = new Thread(() -> {
            try {
                stopped.complete(reader.awaitTermination(Long.MAX_VALUE, TimeUnit.NANOSECONDS));
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                stopped.complete(false);
            }
        }, "qc-usb-reader-teardown");
        waiter.setDaemon(true);
        waiter.start();
        pending = pending.thenCombine(stopped, (previous, current) -> previous && current);
    }

    boolean await(long timeoutMs) throws InterruptedException {
        CompletableFuture<Boolean> current;
        synchronized (this) { current = pending; }
        try {
            return current.get(timeoutMs, TimeUnit.MILLISECONDS);
        } catch (ExecutionException | TimeoutException error) {
            return false;
        }
    }
}
