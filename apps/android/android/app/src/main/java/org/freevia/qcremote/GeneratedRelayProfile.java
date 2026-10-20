// Generated from public contract snapshots and private maintainer specifications; do not edit by hand. The generator is kept in the private repository.
package org.freevia.qcremote;

final class GeneratedRelayProfile {
    static final String INVALID_REQUEST_TEXT = "Invalid relay request.";
    static final String INVALID_REQUEST_ID_TEXT = "The request identifier is invalid.";
    static final String REPLAYED_REQUEST_TEXT = "This request identifier was already processed.";
    static final String METHOD_NOT_ALLOWED_TEXT = "The requested method is not in the generated remote action contract.";
    static final String ACCESS_MODE_RESTRICTED_TEXT = "The requested operation is outside this host's remote access mode.";
    static final String MESSAGE_TOO_LARGE_TEXT = "message too large";
    static final String BINARY_UNSUPPORTED_TEXT = "binary messages are not supported";
    static final String PROTOCOL_VERSION = "qc-relay.v1";
    static final String DEVICE_PAIR_PATH = "/v1/device/pair";
    static final String DEVICE_REPLACEMENT_PATH = "/v1/device/pair/replace-v1";
    static final String REPLACEMENT_ACK_QUERY_PARAMETER = "replacementAck";
    static final String REPLACEMENT_ACK_FRAME_TYPE = "replacement_ack";
    static final String ACCOUNT_DELETION_WEB_PATH = "/account/delete";
    static final String ACCOUNT_DELETION_INFORMATION_URL = "https://freevia.org/qcremote/account/delete/";
    static final String DEVICE_CONNECT_PATH = "/v1/device/connect";
    static final String DEVICE_REVOKE_PATH = "/v1/device/revoke";
    static final String DEVICE_ROSTER_PATH = "/v1/device/roster";
    static final String DEVICE_CONNECTORS_PATH = "/v1/device/connectors";
    static final int CREDENTIAL_TTL_DAYS = 0;
    static final long MAXIMUM_INVOKE_DEADLINE_MS = 1800000L;
    static final long HOST_APPROVAL_TIMEOUT_MS = 120000L;
    static final int PAIRING_CODE_MINIMUM_LENGTH = 6;
    static final int PAIRING_CODE_MAXIMUM_LENGTH = 128;
    static final int MINIMUM_CREDENTIAL_LENGTH = 24;
    static final int MAX_REQUEST_FRAME_BYTES = 65536;
    static final int MAX_RESULT_FRAME_BYTES = 16777216;
    static final int COMPLETED_REQUEST_CACHE_SIZE = 512;
    static final long READINESS_INTERVAL_MS = 1000L;
    static final long READINESS_GRACE_MS = 1500L;
    static final long PING_INTERVAL_MS = 25000L;
    static final int MAXIMUM_FAILURE_COUNT = 10;
    static final int MAXIMUM_BACKOFF_EXPONENT = 6;
    static final long MAXIMUM_BACKOFF_SECONDS = 60L;
    static final long BACKOFF_JITTER_MS = 1000L;


    static String pairingOwnerRefusal(boolean alreadyPaired, String heldValue, String incomingValue, boolean rejected) {
        if (rejected) return null;
        String held = heldValue == null ? "" : heldValue.trim();
        String incoming = incomingValue == null ? "" : incomingValue.trim();
        if (!held.isEmpty() && !incoming.isEmpty() && !held.equalsIgnoreCase(incoming)) return "PAIRING_OWNER_CHANGED";
        if (!held.isEmpty() && incoming.isEmpty()) return "PAIRING_OWNER_UNVERIFIED";
        if (alreadyPaired && held.isEmpty()) return "PAIRING_OWNER_UNKNOWN";
        return null;
    }
    static boolean pairingRejectionHttp(int status) { return status == 401 || status == 403; }
    static boolean pairingRejectionClose(int code) { return code == 4001 || code == 4003; }

    private GeneratedRelayProfile() {}
}
