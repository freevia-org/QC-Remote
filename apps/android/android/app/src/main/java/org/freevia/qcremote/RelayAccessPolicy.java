package org.freevia.qcremote;

import android.content.Context;
import android.content.SharedPreferences;

/** Device-local authority for remote relay writes. Defaults to full control. */
final class RelayAccessPolicy {
    static final String FULL = GeneratedRemoteActions.ACCESS_FULL;
    static final String MODIFY = GeneratedRemoteActions.ACCESS_MODIFY;
    static final String PERFORMANCE = GeneratedRemoteActions.ACCESS_PERFORMANCE;
    static final String READ_ONLY = GeneratedRemoteActions.ACCESS_READ_ONLY;
    private static final String PREFS = "qc_relay_access";
    private static final String MODE = "mode";

    static String mode(Context context) {
        String value = preferences(context).getString(MODE, FULL);
        return READ_ONLY.equals(value) || PERFORMANCE.equals(value) || MODIFY.equals(value) ? value : FULL;
    }

    static void setMode(Context context, String mode) {
        if (!GeneratedRemoteActions.isAccessMode(mode)) {
            throw new IllegalArgumentException("Access mode must be read-only, performance, modify, or full.");
        }
        preferences(context).edit().putString(MODE, mode).apply();
    }

    static boolean permits(Context context, String method) {
        String mode = mode(context);
        if (FULL.equals(mode)) return GeneratedRemoteActions.contains(method);
        if (MODIFY.equals(mode)) return RelayProtocol.isReadOnly(method)
            || GeneratedRemoteActions.isPerformance(method) || GeneratedRemoteActions.isModify(method);
        if (PERFORMANCE.equals(mode)) return RelayProtocol.isReadOnly(method)
            || GeneratedRemoteActions.isPerformance(method);
        return RelayProtocol.isReadOnly(method);
    }

    private static SharedPreferences preferences(Context context) {
        return context.getApplicationContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    private RelayAccessPolicy() {}
}
