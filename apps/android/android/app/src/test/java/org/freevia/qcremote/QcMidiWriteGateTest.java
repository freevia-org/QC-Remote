package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.fail;
import java.util.ArrayList;
import java.util.List;
import org.junit.Test;

public class QcMidiWriteGateTest {
    private static int send(long planned, long current, boolean ready, String suspension, List<String> calls) {
        return QcMidiWriteGate.send(planned, current, ready, suspension,
            () -> calls.add("native-plan"), () -> calls.add("admission"),
            () -> { calls.add("transfer"); return 4; });
    }

    @Test public void queuedCommandCannotBorrowReplacementSession() {
        List<String> calls = new ArrayList<>();
        try { send(1, 2, true, null, calls); fail("Old MIDI must not reach the new session"); }
        catch (RelayException error) { assertEquals("STALE_STATE", error.code); }
        assertEquals(List.of(), calls);
    }

    @Test public void queuedCommandCannotWriteWhileDeviceSuspends() {
        List<String> calls = new ArrayList<>();
        try { send(1, 1, true, "Backup in progress", calls); fail("Suspended MIDI must not dispatch"); }
        catch (RelayException error) { assertEquals(QcSuspensionGate.CODE, error.code); }
        assertEquals(List.of(), calls);
    }

    @Test public void disconnectedOrUnsynchronizedSessionCannotDispatch() {
        List<String> calls = new ArrayList<>();
        try { send(1, 1, false, null, calls); fail("Disconnected MIDI must not dispatch"); }
        catch (RelayException error) { assertEquals("NOT_CONNECTED", error.code); }
        assertEquals(List.of(), calls);
    }

    @Test public void expiredNativePlanCannotAdmitOrTransfer() {
        List<String> calls = new ArrayList<>();
        try {
            QcMidiWriteGate.send(1, 1, true, null,
                () -> { throw new IllegalStateException("Expired native plan"); },
                () -> calls.add("admission"), () -> { calls.add("transfer"); return 4; });
            fail("Expired plan must stop the transfer");
        } catch (IllegalStateException expected) { assertEquals("Expired native plan", expected.getMessage()); }
        assertEquals(List.of(), calls);
    }

    @Test public void liveCommandValidatesAndAdmitsBeforeSingleTransfer() {
        List<String> calls = new ArrayList<>();
        assertEquals(4, send(1, 1, true, null, calls));
        assertEquals(List.of("native-plan", "admission", "transfer"), calls);
    }
}
