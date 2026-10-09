package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;

/** Serializes each persistent mutation from preflight through final verification. */
final class QcPersistentMutationGate {
    interface Operation<T> {
        CompletableFuture<T> start() throws Exception;
    }

    private CompletableFuture<Void> tail = CompletableFuture.completedFuture(null);

    synchronized <T> CompletableFuture<T> submit(Operation<T> operation) {
        CompletableFuture<T> scheduled = tail
            .handle((ignored, priorError) -> null)
            .thenCompose(ignored -> {
                try {
                    return operation.start();
                } catch (Exception error) {
                    CompletableFuture<T> failed = new CompletableFuture<>();
                    failed.completeExceptionally(error);
                    return failed;
                }
            });
        tail = scheduled.handle((ignored, error) -> null);
        return scheduled;
    }
}
