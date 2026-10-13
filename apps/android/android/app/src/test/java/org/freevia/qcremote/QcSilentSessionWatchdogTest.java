package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import org.junit.Test;

public class QcSilentSessionWatchdogTest {
    @Test public void oldTimerCannotRetireReplacementWithSameMessageCount() {
        AtomicLong generation = new AtomicLong(1);
        AtomicLong messages = new AtomicLong(1);
        AtomicInteger retired = new AtomicInteger();
        Runnable oldTimer = new QcSilentSessionWatchdog(1, 1, generation::get,
            messages::get, () -> true, retired::incrementAndGet);
        generation.set(2);
        Runnable replacementTimer = new QcSilentSessionWatchdog(2, 1, generation::get,
            messages::get, () -> true, retired::incrementAndGet);
        oldTimer.run();
        assertEquals(0, retired.get());
        replacementTimer.run();
        assertEquals(1, retired.get());
    }

    @Test public void observedStateMakesCurrentSessionTimerHarmless() {
        AtomicLong messages = new AtomicLong(1);
        AtomicInteger retired = new AtomicInteger();
        Runnable timer = new QcSilentSessionWatchdog(1, 1, () -> 1,
            messages::get, () -> true, retired::incrementAndGet);
        messages.incrementAndGet();
        timer.run();
        assertEquals(0, retired.get());
    }

    @Test public void disconnectedSessionIsNotRetiredAgain() {
        AtomicBoolean ready = new AtomicBoolean(true);
        AtomicInteger retired = new AtomicInteger();
        Runnable timer = new QcSilentSessionWatchdog(1, 1, () -> 1,
            () -> 1, ready::get, retired::incrementAndGet);
        ready.set(false);
        timer.run();
        assertEquals(0, retired.get());
    }
}
