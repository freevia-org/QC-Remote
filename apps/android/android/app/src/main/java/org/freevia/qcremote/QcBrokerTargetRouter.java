package org.freevia.qcremote;

import java.util.concurrent.CompletableFuture;
import java.util.function.Supplier;

/** The selected Demo route is closed when unavailable, never replaced by USB. */
final class QcBrokerTargetRouter {
    static <T> CompletableFuture<T> invoke(boolean demoSelected, QcBrokerTarget target,
            Supplier<CompletableFuture<T>> demo, Supplier<CompletableFuture<T>> physical) {
        if (demoSelected) {
            if (target == null || !"demo".equals(target.source)) {
                CompletableFuture<T> unavailable = new CompletableFuture<>();
                unavailable.completeExceptionally(new RelayException("NOT_CONNECTED",
                    "The selected Demo provider is unavailable. Select the physical device explicitly to use USB."));
                return unavailable;
            }
            return demo.get();
        }
        if (target != null && !"physical".equals(target.source)) {
            CompletableFuture<T> superseded = new CompletableFuture<>();
            superseded.completeExceptionally(new RelayException("TARGET_SUPERSEDED", "The selected broker target changed."));
            return superseded;
        }
        return physical.get();
    }
}
