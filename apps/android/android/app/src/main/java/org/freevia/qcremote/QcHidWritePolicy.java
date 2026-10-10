package org.freevia.qcremote;

import java.util.function.Consumer;

/** Classifies Android SET_REPORT outcomes without treating the QC's status STALL as failure. */
final class QcHidWritePolicy {
    enum Outcome { DELIVERED, EXPECTED_STATUS_STALL, TERMINAL }

    private QcHidWritePolicy() {}

    static void validateDispatch(boolean began, Runnable validate, Consumer<String> terminalFailure) {
        try {
            validate.run();
        } catch (RuntimeException error) {
            if (began) terminalFailure.accept(
                "QC native frame was partially delivered before dispatch was revoked: " + error.getMessage());
            throw error;
        }
    }

    static Outcome classify(
        int result,
        int reportLength,
        boolean deviceAttached,
        boolean sameConnectionGeneration,
        boolean readerTerminated
    ) {
        if (!deviceAttached) return Outcome.TERMINAL;
        if (!sameConnectionGeneration) return Outcome.TERMINAL;
        if (readerTerminated) return Outcome.TERMINAL;
        if (result < 0) return Outcome.EXPECTED_STATUS_STALL;
        return result == reportLength ? Outcome.DELIVERED : Outcome.TERMINAL;
    }

    static Outcome classifyAndPropagate(
        int result,
        int reportLength,
        boolean deviceAttached,
        boolean sameConnectionGeneration,
        boolean readerTerminated,
        Consumer<String> terminalFailure
    ) {
        Outcome outcome = classify(
            result, reportLength, deviceAttached, sameConnectionGeneration, readerTerminated);
        if (outcome == Outcome.TERMINAL) {
            String detail = !deviceAttached
                ? "Quad Cortex USB detached during HID write."
                : !sameConnectionGeneration
                    ? "Quad Cortex USB session changed during HID write."
                    : readerTerminated
                        ? "Quad Cortex receive lane terminated during HID write."
                        : "USB HID write returned " + result + " of " + reportLength + " bytes.";
            terminalFailure.accept(detail);
        }
        return outcome;
    }
}
