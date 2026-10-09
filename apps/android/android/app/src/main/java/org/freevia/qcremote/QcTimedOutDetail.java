package org.freevia.qcremote;

/**
 * The detail of a write whose confirming state did not arrive in time: the frames were sent
 * and the required observations were not all seen in the bound, so nothing is claimed. A
 * recall, bank navigation or reload names the conditions it waited for, as the desktop broker
 * does (PresetRecallPlan::timeout_detail), and never asserts which one was missing: the
 * timed-out action carries no evidence of that. Its plan detail names the requested action,
 * never an outcome. Kept free of Android and Capacitor types so a plain JVM test can load it.
 */
final class QcTimedOutDetail {
    private QcTimedOutDetail() {}

    static String of(String method, String detail, long timeoutMs) {
        switch (method) {
            case "device.recallPreset":
            case "device.navigateBank":
                return detail + "; the frame was sent, but within " + timeoutMs
                    + " ms the host did not observe all of the required conditions together (a newer position update naming the requested preset address and a newer full preset publication), so the effect is not verified";
            case "device.reloadPreset":
                return detail + "; the frame was sent, but within " + timeoutMs
                    + " ms the host did not observe all of the required conditions together (a newer position update naming the active preset address, a newer full preset publication and a clean flag), so the discard is not verified";
            default:
                return detail + "; the QC pushed no confirming state within " + timeoutMs + " ms, so the effect is not verified";
        }
    }
}
