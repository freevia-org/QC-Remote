package org.freevia.qcremote;

import java.util.function.IntSupplier;

/** Dispatch fence for a queued MIDI write; the caller holds the USB session monitor. */
final class QcMidiWriteGate {
    static int send(long plannedGeneration, long currentGeneration, boolean ready,
            String suspensionRefusal, Runnable validateNativePlan, Runnable admitMutation,
            IntSupplier transfer) {
        if (plannedGeneration != currentGeneration)
            throw new RelayException("STALE_STATE", "The USB session changed after MIDI planning; no mutation was sent.");
        if (!ready)
            throw new RelayException("NOT_CONNECTED", "Quad Cortex USB is not synchronized before the MIDI write.");
        if (suspensionRefusal != null)
            throw new RelayException(QcSuspensionGate.CODE, suspensionRefusal);
        validateNativePlan.run();
        admitMutation.run();
        return transfer.getAsInt();
    }

    private QcMidiWriteGate() {}
}
