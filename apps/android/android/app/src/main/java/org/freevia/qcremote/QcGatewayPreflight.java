package org.freevia.qcremote;

import org.json.JSONObject;

/**
 * Selects the shared Rust plan's authoritative preflight observation. There is exactly one
 * source, the decoded state the session folds from what the QC pushed; no preflight sends a
 * READ.
 */
final class QcGatewayPreflight {
    interface SnapshotReader {
        JSONObject read() throws Exception;
    }

    private QcGatewayPreflight() {}

    static JSONObject authoritativeObservation(String source, SnapshotReader snapshotReader) throws Exception {
        if ("decodedSnapshot".equals(source)) return snapshotReader.read();
        throw new IllegalStateException(
            "Native QC gateway plan has no valid preflight observation source.");
    }
}
