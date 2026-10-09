package org.freevia.qcremote;

import java.util.concurrent.atomic.AtomicBoolean;

/** Single-flight ownership for RemoteControl screenshots, whose wire reply has no request ID. */
final class QcCaptureScreenGate {
    private final AtomicBoolean owned = new AtomicBoolean();

    boolean tryAcquire() { return owned.compareAndSet(false, true); }

    void release() { owned.set(false); }
}
