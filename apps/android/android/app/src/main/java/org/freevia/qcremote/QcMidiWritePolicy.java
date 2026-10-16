package org.freevia.qcremote;

import java.util.function.IntSupplier;

/** A queued MIDI control may reach only its original USB connection and endpoint. */
final class QcMidiWritePolicy {
    private QcMidiWritePolicy() {}

    /** Caller holds the same transport monitor as teardown for the bounded write. */
    static int writeIfCurrent(long admittedGeneration, long currentGeneration,
            Object admittedConnection, Object currentConnection,
            Object admittedEndpoint, Object currentEndpoint, IntSupplier write) {
        if (admittedGeneration != currentGeneration || admittedConnection == null
            || admittedConnection != currentConnection || admittedEndpoint == null
            || admittedEndpoint != currentEndpoint) {
            throw new RelayException("NOT_CONNECTED", "The USB session changed before the queued MIDI control; nothing was sent.");
        }
        return write.getAsInt();
    }
}
