package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.fail;

import java.util.ArrayList;
import java.util.List;
import org.junit.Test;

public class QcHidWritePolicyTest {
    @Test
    public void rejectedDispatchBeforeFirstReportLeavesTheSessionAlone() {
        List<String> terminal = new ArrayList<>();
        RuntimeException rejected = new IllegalStateException("expired queued operation");
        try {
            QcHidWritePolicy.validateDispatch(false, () -> { throw rejected; }, terminal::add);
            fail("The rejected frame must not proceed to native I/O");
        } catch (RuntimeException actual) { assertSame(rejected, actual); }
        assertEquals(0, terminal.size());
    }

    @Test
    public void revokedDispatchAfterFirstReportPropagatesTerminalPartialDelivery() {
        List<String> terminal = new ArrayList<>();
        RuntimeException rejected = new IllegalStateException("session changed");
        try {
            QcHidWritePolicy.validateDispatch(true, () -> { throw rejected; }, terminal::add);
            fail("The remaining fragments must not proceed to native I/O");
        } catch (RuntimeException actual) { assertSame(rejected, actual); }
        assertEquals(1, terminal.size());
        assertEquals("QC native frame was partially delivered before dispatch was revoked: session changed", terminal.get(0));
    }

    @Test
    public void twoFragmentStatusStallsRemainDelivered() {
        List<String> terminal = new ArrayList<>();
        assertEquals(
            QcHidWritePolicy.Outcome.EXPECTED_STATUS_STALL,
            QcHidWritePolicy.classifyAndPropagate(-1, 129, true, true, false, terminal::add));
        assertEquals(
            QcHidWritePolicy.Outcome.EXPECTED_STATUS_STALL,
            QcHidWritePolicy.classifyAndPropagate(-1, 129, true, true, false, terminal::add));
        assertEquals(0, terminal.size());
    }

    @Test
    public void detachIsTerminalAndPropagatesRecoverySignal() {
        List<String> terminal = new ArrayList<>();
        assertEquals(
            QcHidWritePolicy.Outcome.TERMINAL,
            QcHidWritePolicy.classifyAndPropagate(-1, 129, false, true, false, terminal::add));
        assertEquals(1, terminal.size());
        assertEquals("Quad Cortex USB detached during HID write.", terminal.get(0));
    }

    @Test
    public void receiveLaneCessationIsTerminalAndPropagatesRecoverySignal() {
        List<String> terminal = new ArrayList<>();
        assertEquals(
            QcHidWritePolicy.Outcome.TERMINAL,
            QcHidWritePolicy.classifyAndPropagate(-1, 129, true, true, true, terminal::add));
        assertEquals(1, terminal.size());
        assertEquals("Quad Cortex receive lane terminated during HID write.", terminal.get(0));
    }

    @Test
    public void shortWriteIsTerminalButCompleteWriteIsDelivered() {
        List<String> terminal = new ArrayList<>();
        assertEquals(
            QcHidWritePolicy.Outcome.DELIVERED,
            QcHidWritePolicy.classifyAndPropagate(129, 129, true, true, false, terminal::add));
        assertEquals(
            QcHidWritePolicy.Outcome.TERMINAL,
            QcHidWritePolicy.classifyAndPropagate(64, 129, true, true, false, terminal::add));
        assertEquals(1, terminal.size());
    }
}
