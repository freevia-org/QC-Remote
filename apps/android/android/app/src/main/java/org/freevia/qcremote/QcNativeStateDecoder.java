package org.freevia.qcremote;

import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import org.json.JSONArray;
import org.json.JSONObject;

/** Narrow JNI facade over the shared Rust protocol/state engine. */
final class QcNativeStateDecoder implements AutoCloseable {
    static { System.loadLibrary("quadcortex_android"); }

    static final int REPORT_SIZE = nativeReportSize();
    static final int OUT_REPORT_ID = nativeOutboundReportId();
    static final int IN_REPORT_ID = nativeInboundReportId();

    static final class EncodedMessage {
        final int messageType;
        final byte[] payload;

        EncodedMessage(int messageType, byte[] payload) {
            this.messageType = messageType;
            this.payload = payload;
        }
    }

    static final class DecodedFrame {
        final String observationTicket;
        final QcObservationFence observationFence;
        final int messageType;
        final byte[] payload;
        final List<JSObject> states;
        final JSObject tempoClock;
        final LifecycleStatus lifecycle;
        final boolean forwardingOnly;
        DecodedFrame(String observationTicket, int messageType, byte[] payload,
            List<JSObject> states, JSObject tempoClock, LifecycleStatus lifecycle, boolean forwardingOnly) {
            this.observationTicket = observationTicket;
            this.observationFence = new QcObservationFence(observationTicket);
            this.messageType = messageType;
            this.payload = payload;
            this.states = states;
            this.tempoClock = tempoClock;
            this.lifecycle = lifecycle;
            this.forwardingOnly = forwardingOnly;
        }
        static DecodedFrame decode(byte[] decoded) {
            if (decoded.length == 0) return null;
            if (decoded.length < 6) throw new IllegalStateException("Native QC framing returned no observation header.");
            int messageType = (decoded[0] & 0xff) | ((decoded[1] & 0xff) << 8);
            long headerLength = java.nio.ByteBuffer.wrap(decoded, 2, 4)
                .order(java.nio.ByteOrder.LITTLE_ENDIAN).getInt() & 0xffffffffL;
            if (headerLength == 0 || headerLength > decoded.length - 6)
                throw new IllegalStateException("Native QC framing returned an invalid decoded header length.");
            int payloadOffset = 6 + (int) headerLength;
            try {
                JSONObject header = new JSONObject(new String(decoded, 6, (int) headerLength,
                    java.nio.charset.StandardCharsets.UTF_8));
                String ticket = header.getJSONObject("observation").toString();
                if (header.optBoolean("forwardingOnly", false)) {
                    return new DecodedFrame(ticket, messageType, Arrays.copyOfRange(decoded, payloadOffset, decoded.length),
                        java.util.Collections.emptyList(), null, null, true);
                }
                JSONArray encodedStates = header.getJSONArray("states");
                List<JSObject> states = new ArrayList<>();
                for (int index = 0; index < encodedStates.length(); index++) {
                    states.add(JSObject.fromJSONObject(encodedStates.getJSONObject(index)));
                }
                return new DecodedFrame(ticket, messageType, Arrays.copyOfRange(decoded, payloadOffset, decoded.length),
                    states,
                    header.isNull("tempoClock") ? null : JSObject.fromJSONObject(header.getJSONObject("tempoClock")),
                    new LifecycleStatus(header.getJSONObject("lifecycle").toString()), false);
            } catch (Exception error) {
                throw new IllegalStateException("Native QC framing returned an invalid decoded observation.", error);
            }
        }
    }

    static final class PlannedGatewayWrite {
        final String sessionFence;
        final String method;
        final String detail;
        final String verificationJson;
        final java.util.Set<String> observedStateKinds;
        final boolean midi;
        final boolean noOp;
        final int controller;
        final int value;
        final boolean realtime;
        final long interMessageIntervalMs;
        final long confirmationTimeoutMs;
        final String preflightMethod;
        final String preflightObservationSource;
        final String readbackMethod;
        final JSONObject catalogPolicy;
        final List<EncodedMessage> messages;

        PlannedGatewayWrite(
            String sessionFence, String method, String detail, String verificationJson, java.util.Set<String> observedStateKinds, boolean noOp, boolean midi, int controller, int value,
            boolean realtime, long interMessageIntervalMs,
            long confirmationTimeoutMs,
            String preflightMethod, String preflightObservationSource,
            String readbackMethod, JSONObject catalogPolicy, List<EncodedMessage> messages
        ) {
            this.sessionFence = sessionFence;
            this.method = method;
            this.detail = detail;
            this.verificationJson = verificationJson;
            this.observedStateKinds = java.util.Collections.unmodifiableSet(new java.util.HashSet<>(observedStateKinds));
            this.midi = midi;
            this.noOp = noOp;
            this.controller = controller;
            this.value = value;
            this.realtime = realtime;
            this.interMessageIntervalMs = interMessageIntervalMs;
            this.confirmationTimeoutMs = confirmationTimeoutMs;
            this.preflightMethod = preflightMethod;
            this.preflightObservationSource = preflightObservationSource;
            this.readbackMethod = readbackMethod;
            this.catalogPolicy = catalogPolicy;
            this.messages = messages;
        }

        PlannedGatewayWrite withSessionFence(String context) {
            return new PlannedGatewayWrite(context, method, detail, verificationJson, observedStateKinds, noOp, midi, controller, value,
                realtime, interMessageIntervalMs, confirmationTimeoutMs,
                preflightMethod, preflightObservationSource,
                readbackMethod, catalogPolicy, messages);
        }
    }

    static final class GatewayWritePreflight {
        final PlannedGatewayWrite plan;
        final boolean alreadyMatched;
        final JSONObject observation;

        GatewayWritePreflight(PlannedGatewayWrite plan, boolean alreadyMatched, JSONObject observation) {
            this.plan = plan;
            this.alreadyMatched = alreadyMatched;
            this.observation = observation;
        }
    }

    static final class PlannedGatewayRead {
        final String sessionFence;
        final String sessionEpoch;
        final int responseType;
        final long timeoutMs;
        final String projectionJson;
        final String followupMethod;
        final List<EncodedMessage> messages;

        PlannedGatewayRead(
            String sessionFence, String sessionEpoch, int responseType, long timeoutMs, String projectionJson,
            String followupMethod, List<EncodedMessage> messages
        ) {
            this.sessionFence = sessionFence;
            this.sessionEpoch = sessionEpoch;
            this.responseType = responseType;
            this.timeoutMs = timeoutMs;
            this.projectionJson = projectionJson;
            this.followupMethod = followupMethod;
            this.messages = messages;
        }
    }

    private long handle = nativeCreate();

    static final class MetadataResult {
        final String observationTicket;
        final List<JSObject> states;
        final String error;
        MetadataResult(String value) throws Exception {
            org.json.JSONObject object = new org.json.JSONObject(value);
            observationTicket = object.getString("observationTicket");
            states = objects(object.getJSONArray("states").toString());
            error = object.isNull("error") ? null : object.getString("error");
        }
    }

    boolean queueModelRepo(String ticket, byte[] payload) {
        return Boolean.parseBoolean(nativeQueueModelRepo(requireHandle(), ticket, payload));
    }

    boolean modelRepoReady() {
        return Boolean.parseBoolean(nativeModelRepoReady(requireHandle()));
    }

    MetadataResult parseNextModelRepo(long nowMs) throws Exception {
        String result = nativeParseNextModelRepo(requireHandle(), nowMs);
        return "null".equals(result) ? null : new MetadataResult(result);
    }

    static final class StatePublication {
        final String observationTicket;
        final QcObservationFence observationFence;
        final long observedAt;
        final List<JSObject> states;
        final JSObject tempoClock;

        StatePublication(JSONObject value) throws Exception {
            observationTicket = value.getJSONObject("observation").toString();
            observationFence = new QcObservationFence(observationTicket);
            observedAt = value.getLong("observedAt");
            JSONArray values = value.getJSONArray("states");
            states = new ArrayList<>();
            for (int index = 0; index < values.length(); index++) {
                states.add(JSObject.fromJSONObject(values.getJSONObject(index)));
            }
            tempoClock = value.isNull("tempoClock") ? null : new JSObject(value.getJSONObject("tempoClock").toString());
        }
    }

    boolean statePublicationsReady() {
        return Boolean.parseBoolean(nativeStatePublicationsReady(requireHandle()));
    }

    List<StatePublication> takeStatePublications() throws Exception {
        JSONArray values = new JSONArray(nativeTakeStatePublications(requireHandle()));
        List<StatePublication> publications = new ArrayList<>();
        for (int index = 0; index < values.length(); index++) publications.add(new StatePublication(values.getJSONObject(index)));
        return publications;
    }

    void assertExpectedPreset(org.json.JSONObject params) throws Exception {
        nativeAssertExpectedPreset(requireHandle(), params.toString());
    }

    JSObject blockDetails(int row, int column, org.json.JSONObject params) throws Exception {
        try {
            return new JSObject(nativeBlockDetails(requireHandle(), row, column, params.toString()));
        } catch (Exception error) {
            String message = error.getMessage();
            for (String code : new String[] {"STATE_UNAVAILABLE", "UNSAVED_STATE_UNCONFIRMED", "UNSAVED_STATE_NOT_YET_OBSERVED"}) {
                if (message != null && message.startsWith(code + ": ")) {
                    throw new RelayException(code, message.substring(code.length() + 2));
                }
            }
            throw error;
        }
    }

    JSObject laneControlDetails(int row, String control, org.json.JSONObject params) throws Exception {
        return new JSObject(nativeLaneControlDetails(requireHandle(), row, control, params.toString()));
    }

    JSObject snapshot() throws Exception { return new JSObject(nativeSnapshot(requireHandle())); }

    /** A settings family the QC publishes rather than answers, from the shared fold. */
    JSObject publishedSettings(int messageType) throws Exception {
        return new JSObject(nativePublishedSettings(requireHandle(), messageType));
    }

    /** The observation the shared plan's decodedSnapshot preflight reads for this method. */
    JSObject preflightObservation(String method) throws Exception {
        return new JSObject(nativePreflightObservation(requireHandle(), method));
    }
    JSObject currentPreset() throws Exception { return new JSObject(nativeCurrentPreset(requireHandle())); }
    JSObject currentPresetObservation() throws Exception { return new JSObject(nativeCurrentPresetObservation(requireHandle())); }
    long fullPresetRevision() { return nativeFullPresetRevision(requireHandle()); }
    JSObject modelList() throws Exception { return new JSObject(nativeModelList(requireHandle())); }

    JSObject retainedCatalog(String context, String method, String key, long nowMs) throws Exception {
        return new JSObject(nativeRetainedCatalog(requireHandle(), context, method, key, nowMs));
    }
    JSObject mergeExpectedState(JSObject params, JSObject expected) throws Exception {
        return new JSObject(nativeMergeExpectedState(params.toString(), expected.toString()));
    }

    int modelCount() { return nativeModelCount(requireHandle()); }

    long sessionOpened(long nowMs) { return nativeSessionOpened(requireHandle(), nowMs); }
    boolean sessionConnected() { return nativeSessionConnected(requireHandle()) == 1; }
    boolean sessionSynchronized() { return nativeSessionSynchronized(requireHandle()) == 1; }
    /** The shared owner's suspension record; {@code active} is null unless suspended. */
    JSObject suspension() throws Exception { return new JSObject(nativeSuspension(requireHandle())); }
    /** The refusal for a request that would reach the device, or null when not suspended. */
    String suspensionRefusal() {
        String refusal = nativeSuspensionRefusal(requireHandle());
        return refusal == null || refusal.isEmpty() ? null : refusal;
    }
    /** The reason of an ended suspension that requires a full session resync, once. */
    String takeSuspensionResync() {
        String reason = nativeTakeSuspensionResync(requireHandle());
        return reason == null || reason.isEmpty() ? null : reason;
    }
    boolean sessionShouldKeepalive(long nowMs) {
        return nativeSessionShouldKeepalive(requireHandle(), nowMs) == 1;
    }

    static final class LifecycleStatus {
        final String phase;
        final String error;
        final boolean sessionValidated;
        LifecycleStatus(String raw) {
            try {
                JSONObject status = new JSONObject(raw);
                phase = status.getString("phase");
                error = status.isNull("error") ? null : status.getString("error");
                sessionValidated = status.optBoolean("sessionValidated", false);
            } catch (Exception error) {
                throw new IllegalStateException("Invalid native QC lifecycle status.", error);
            }
        }
    }

    static final class LifecycleBatch {
        final String ticket;
        final String dispatchTicket;
        final List<EncodedMessage> messages;
        LifecycleBatch(JSONObject batch) throws Exception {
            ticket = batch.getJSONObject("ticket").toString();
            dispatchTicket = batch.getJSONObject("dispatch").toString();
            messages = encodedMessages(batch.getJSONArray("messages"));
        }
    }

    static final class HandshakeDecision {
        static final int WAIT = 0;
        static final int SEND = 1;
        static final int TIMED_OUT = 2;

        final int kind;
        final int attempt;
        final long requestId;
        final boolean includeReportId;

        HandshakeDecision(
            int kind, int attempt, long requestId, boolean includeReportId
        ) {
            this.kind = kind;
            this.attempt = attempt;
            this.requestId = requestId;
            this.includeReportId = includeReportId;
        }
    }

    void sessionKeepaliveSent(long nowMs) { nativeSessionKeepaliveSent(requireHandle(), nowMs); }
    void sessionDisconnected(long nowMs) { nativeSessionDisconnected(requireHandle(), nowMs); }
    long sessionScheduleReconnect(long nowMs) {
        return nativeSessionScheduleReconnect(requireHandle(), nowMs);
    }
    boolean sessionReconnectDue(long nowMs) {
        return nativeSessionReconnectDue(requireHandle(), nowMs) == 1;
    }
    void sessionReconnectAttempted(long nowMs) {
        nativeSessionReconnectAttempted(requireHandle(), nowMs);
    }
    boolean sessionTerminalReadFailed() {
        return nativeSessionTerminalReadFailed(requireHandle()) == 1;
    }

    HandshakeDecision handshakeAttempt(long nowMs, String sessionId) {
        try {
            JSONObject decision = new JSONObject(
                nativeHandshakeAttempt(requireHandle(), nowMs, sessionId));
            int kind = decision.getInt("kind");
            if (kind < HandshakeDecision.WAIT || kind > HandshakeDecision.TIMED_OUT)
                throw new IllegalStateException("Native QC handshake decision is invalid.");
            if (kind != HandshakeDecision.SEND)
                return new HandshakeDecision(kind, 0, 0, false);
            return new HandshakeDecision(
                kind, decision.getInt("attempt"), nonNegativeLong(decision, "requestId"),
                decision.getBoolean("includeReportId"));
        } catch (Exception error) {
            throw new IllegalStateException("Native QC handshake decision is invalid.", error);
        }
    }

    boolean startupConnected() { return nativeStartupConnected(requireHandle()) == 1; }
    boolean startupActive() { return nativeStartupActive(requireHandle()) == 1; }

    LifecycleStatus lifecyclePoll(long nowMs) {
        return new LifecycleStatus(nativeLifecyclePoll(requireHandle(), nowMs));
    }

    LifecycleBatch lifecycleNextBatch(long nowMs, long unixTimeMs) throws Exception {
        String raw = nativeLifecycleNextBatch(requireHandle(), nowMs, unixTimeMs);
        return "null".equals(raw) ? null : new LifecycleBatch(new JSONObject(raw));
    }

    boolean lifecycleBatchCurrent(LifecycleBatch batch) {
        return nativeLifecycleBatchCurrent(requireHandle(), batch.ticket) == 1;
    }

    void lifecycleDispatched(LifecycleBatch batch, boolean accepted, long nowMs) {
        // Status is polled again by the writer. Do not apply a stale callback's
        // snapshot after another thread has already replaced the lifecycle.
        nativeLifecycleDispatched(requireHandle(), batch.ticket, accepted ? 1 : 0, nowMs);
    }

    EncodedMessage keepaliveCommand() throws Exception {
        return one("keepalive", new JSObject());
    }

    EncodedMessage backupCommand() throws Exception {
        return one("backup", new JSObject());
    }

    static String validateBackupName(String name) { return nativeValidateBackupName(name); }

    /** Pure bounded inspection; called by backupFileIo after the file has been saved. */
    static JSONObject inspectBackupChecksum(byte[] utf8) throws Exception {
        if (utf8.length > GeneratedBackupChecksum.MAXIMUM_JSON_BYTES)
            return new JSONObject(GeneratedBackupChecksum.jsonForReason("inspection_limit"));
        return new JSONObject(nativeInspectBackupChecksum(utf8));
    }

    List<EncodedMessage> disconnectCommands() throws Exception {
        return commands("disconnect", new JSObject());
    }

    String backupStarted(String operation, long nowMs) {
        return nativeBackupStarted(requireHandle(), operation, nowMs);
    }

    JSObject backupAdvance(String operation, long nowMs) throws Exception {
        return new JSObject(nativeBackupAdvance(requireHandle(), operation, nowMs));
    }

    void claimBackupRecovery(String operation) { nativeClaimBackupRecovery(requireHandle(), operation); }

    void backupCancelled(String operation) { nativeBackupCancelled(requireHandle(), operation); }

    JSObject consumeBackupChunk(String operation, String observation, byte[] payload, String name, long nowMs) throws Exception {
        return new JSObject(nativeConsumeBackupChunk(requireHandle(), operation, observation, payload, name, nowMs));
    }

    PlannedGatewayWrite gatewayPlan(String method, JSObject args) throws Exception {
        JSONObject plan;
        try {
            plan = new JSONObject(nativePlanGatewayWrite(
                requireHandle(), method, args.toString(), System.nanoTime() / 1_000_000L));
        } catch (Exception error) {
            String message = error.getMessage();
            for (String code : new String[] {"STATE_UNAVAILABLE", "UNSAVED_STATE_UNCONFIRMED", "UNSAVED_STATE_NOT_YET_OBSERVED"}) {
                if (message != null && message.startsWith(code + ": ")) {
                    throw new RelayException(code, message.substring(code.length() + 2));
                }
            }
            throw error;
        }
        String lane = plan.getString("lane");
        if (!"hid".equals(lane) && !"midi".equals(lane))
            throw new IllegalStateException("Native QC gateway execution lane is invalid.");
        java.util.Set<String> observedKinds = new java.util.HashSet<>();
        JSONArray nativeKinds = plan.getJSONArray("observedStateKinds");
        for (int i = 0; i < nativeKinds.length(); i++) observedKinds.add(nativeKinds.getString(i));
        return new PlannedGatewayWrite(
            plan.getJSONObject("sessionFence").toString(),
            plan.getString("method"), plan.getString("detail"), plan.getJSONObject("verification").toString(), observedKinds, plan.getBoolean("noOp"),
            "midi".equals(lane), plan.getInt("controller"), plan.getInt("value"),
            plan.getBoolean("realtime"), nonNegativeLong(plan, "interMessageIntervalMs"),
            nonNegativeLong(plan, "confirmationTimeoutMs"),
            nullableString(plan, "preflightMethod"),
            nullableString(plan, "preflightObservationSource"),
            nullableString(plan, "readbackMethod"), plan.optJSONObject("catalogPolicy"),
            encodedMessages(plan.getJSONArray("messages")));
    }

    boolean operationObservationIsFresh(String operation, String observation) {
        return operation != null && observation != null
            && nativeOperationObservationIsFresh(requireHandle(), operation, observation) == 1;
    }
    private static native int nativeOperationObservationIsFresh(long handle, String operation, String observation);

    boolean gatewayWriteEchoMatches(String method, JSONObject params, int messageType, byte[] payload) {
        return nativeGatewayWriteEchoMatches(method, params.toString(), messageType, payload) == 1;
    }

    private static native int nativeGatewayWriteEchoMatches(String method, String params, int messageType, byte[] payload);

    boolean gatewayReadbackMatches(String method, org.json.JSONObject params, org.json.JSONObject response) {
        return nativeGatewayReadbackMatches(method, params.toString(), response.toString()) == 1;
    }

    long gatewayReadbackRetryDelay(String method, int attempt) {
        return nativeGatewayReadbackRetryDelay(method, attempt);
    }

    long gatewayVerificationStarted(
        PlannedGatewayWrite plan, long afterSequence, long startedAtMs
    ) {
        return nativeGatewayVerificationStarted(
            requireHandle(), plan.verificationJson, plan.sessionFence, plan.method, afterSequence,
            startedAtMs, plan.confirmationTimeoutMs);
    }

    /**
     * Advances one write verification. A {@code verified} action for a
     * {@code device.addBlock} of a plugin model also carries the shared
     * {@code pluginBlockReadback}, decided natively against the very state the
     * verification matched.
     */
    JSONObject gatewayVerificationAdvance(
        long verificationId, long observationSequence, long nowMs,
        String method, String verificationJson
    ) throws Exception {
        return new JSONObject(nativeGatewayVerificationAdvance(
            requireHandle(), verificationId, observationSequence, nowMs, method, verificationJson));
    }

    void gatewayVerificationCancelled(long verificationId) {
        nativeGatewayVerificationCancelled(requireHandle(), verificationId);
    }



    GatewayWritePreflight bindGatewayWritePreflight(
        PlannedGatewayWrite plan, JSONObject params, JSONObject response
    ) throws Exception {
        JSONObject bound = new JSONObject(nativeBindGatewayWritePreflight(
            requireHandle(), plan.sessionFence, plan.method, params.toString(), response.toString(), System.nanoTime() / 1_000_000L));
        int decision = bound.getInt("decision");
        if (decision != 1 && decision != 2) throw new IllegalStateException("Invalid native QC preflight decision.");
        return new GatewayWritePreflight(plan.withSessionFence(bound.getJSONObject("context").toString()),
            decision == 2, response);
    }

    JSONObject composeGlobalTempoSettings(JSONObject global, JSONObject preset) throws Exception {
        return new JSONObject(nativeComposeGlobalTempoSettings(global.toString(), preset.toString()));
    }

    static final class PresetWorkflowAction {
        final JSONObject value;
        final List<EncodedMessage> messages;
        PresetWorkflowAction(String json) throws Exception {
            value = new JSONObject(json);
            messages = value.has("messages") ? encodedMessages(value.getJSONArray("messages")) : new ArrayList<>();
        }
    }

    long beginPresetWorkflow(String method, JSONObject args, long nowMs) throws Exception {
        return new JSONObject(nativeBeginPresetWorkflow(requireHandle(), method, args.toString(), nowMs))
            .getLong("preparationId");
    }

    PresetWorkflowAction advancePresetWorkflow(long preparation, long nowMs) throws Exception {
        return new PresetWorkflowAction(nativeAdvancePresetWorkflow(requireHandle(), preparation, nowMs));
    }

    void cancelPresetPreparation(long preparation) {
        nativeCancelPresetPreparation(requireHandle(), preparation);
    }

    PlannedGatewayRead gatewayRead(String method, JSObject args) throws Exception {
        return gatewayRead(method, args, "", System.nanoTime() / 1_000_000L);
    }

    String beginGatewayReadScope(String method, JSONObject args, long startedMs) {
        return beginGatewayReadScope(method, args, "", startedMs);
    }

    String beginGatewayReadScope(String method, JSONObject args, String context, long startedMs) {
        return nativeBeginGatewayReadScope(requireHandle(), method, args.toString(), context, startedMs);
    }

    String beginGatewayReadbackScope(String context, long startedMs, long timeoutMs) {
        return nativeBeginGatewayReadbackScope(requireHandle(), context, startedMs, timeoutMs);
    }

    long gatewayReadScopeRemaining(String scope, long nowMs) {
        return nativeGatewayReadScopeRemaining(requireHandle(), scope, nowMs);
    }

    PlannedGatewayRead gatewayRead(String method, JSObject args, String scope, long startedMs) throws Exception {
        JSONObject plan = new JSONObject(nativePlanGatewayRead(requireHandle(), method, args.toString(), scope, startedMs));
        return new PlannedGatewayRead(
            plan.getJSONObject("sessionFence").toString(),
            plan.getJSONObject("sessionEpoch").toString(),
            plan.getInt("responseType"), nonNegativeLong(plan, "timeoutMs"),
            plan.getJSONObject("projection").toString(),
            nullableString(plan, "followupMethod"),
            encodedMessages(plan.getJSONArray("messages")));
    }

    JSObject observeGatewayRead(PlannedGatewayRead plan, String observationTicket, int messageType, byte[] payload, long startedAtMs, long nowMs) throws Exception {
        return new JSObject(nativeObserveGatewayRead(requireHandle(), observationTicket, plan.sessionFence, plan.projectionJson, payload, plan.responseType, messageType, startedAtMs, plan.timeoutMs, nowMs));
    }

    static final int RESPONSE_UNRELATED = 0;
    static final int RESPONSE_MATCHED = 1;
    static final int RESPONSE_EXPIRED = 2;

    int gatewayResponseDecision(PlannedGatewayRead plan, int messageType, byte[] payload, long startedAtMs, long nowMs) {
        return nativeGatewayResponseDecision(
            plan.projectionJson, plan.responseType, messageType, payload, startedAtMs, plan.timeoutMs, nowMs);
    }

    boolean readEpochCurrent(PlannedGatewayRead plan) {
        return nativeReadEpochCurrent(requireHandle(), plan.sessionEpoch) == 1;
    }

    boolean requestExpired(long startedAtMs, long timeoutMs, long nowMs) {
        return nativeRequestExpired(startedAtMs, timeoutMs, nowMs) == 1;
    }

    String operationTicket(long nowMs, long timeoutMs) {
        return nativePlanOperation(requireHandle(), "", nowMs, timeoutMs);
    }

    String catalogContext(String context) {
        return nativeCatalogContext(requireHandle(), context == null ? "" : context);
    }


    String operationTicket(String sessionFence, long nowMs, long timeoutMs) {
        if (sessionFence == null || sessionFence.isEmpty())
            throw new IllegalStateException("Native QC plan has no session provenance.");
        return nativePlanOperation(requireHandle(), sessionFence, nowMs, timeoutMs);
    }

    String prepareDispatch(String operationTicket, long nowMs) {
        return nativePrepareDispatch(requireHandle(), operationTicket == null ? "" : operationTicket, nowMs);
    }

    long operationRemaining(String operationTicket, long nowMs) {
        return nativeOperationRemaining(requireHandle(), operationTicket, nowMs);
    }

    String prepareDelivery(String dispatch, int messageType, int reportCount, long nowMs) {
        return nativePrepareDelivery(requireHandle(), dispatch, messageType, reportCount, nowMs);
    }

    void deliveryAdmitted(String ticket, long nowMs) { nativeDeliveryEvent(requireHandle(), ticket, 0, 0, nowMs); }
    void beginDeliveryReport(String ticket, int index, long nowMs) { nativeDeliveryEvent(requireHandle(), ticket, 1, index, nowMs); }
    void finishDeliveryReport(String ticket, int index, QcHidWritePolicy.Outcome outcome, long nowMs) {
        int event = outcome == QcHidWritePolicy.Outcome.TERMINAL ? 4
            : outcome == QcHidWritePolicy.Outcome.EXPECTED_STATUS_STALL ? 3 : 2;
        nativeDeliveryEvent(requireHandle(), ticket, event, index, nowMs);
    }
    void requireDeliveryComplete(String ticket, long nowMs) { nativeDeliveryEvent(requireHandle(), ticket, 6, 0, nowMs); }
    void stopPendingDelivery(String ticket, long nowMs) {
        try { nativeDeliveryEvent(requireHandle(), ticket, 5, 0, nowMs); }
        catch (RuntimeException ignored) {
            // Teardown may already have revoked or destroyed this native owner.
            // A cleanup callback must not replace the original transfer error.
        }
    }

    List<byte[]> encodeReports(EncodedMessage message, boolean includeReportId, String dispatchTicket, long nowMs) {
        int reportSize = includeReportId ? REPORT_SIZE : REPORT_SIZE - 1;
        byte[] encoded = nativeEncodeReports(
            requireHandle(), message.messageType, message.payload, includeReportId ? 1 : 0,
            dispatchTicket == null ? "" : dispatchTicket, nowMs);
        if (encoded.length == 0 || encoded.length % reportSize != 0) {
            throw new IllegalStateException("Native QC framing returned an invalid report sequence.");
        }
        List<byte[]> reports = new ArrayList<>(encoded.length / reportSize);
        for (int offset = 0; offset < encoded.length; offset += reportSize) {
            reports.add(Arrays.copyOfRange(encoded, offset, offset + reportSize));
        }
        return reports;
    }

    long captureReportEpoch(long connection) {
        return nativeCaptureReportEpoch(requireHandle(), connection);
    }

    DecodedFrame pushReport(long connection, long reportEpoch, byte[] report, long receivedAtMs) {
        byte[] decoded = nativePushReport(requireHandle(), connection, reportEpoch, report, receivedAtMs);
        try { return DecodedFrame.decode(decoded); }
        finally { Arrays.fill(decoded, (byte) 0); }
    }

    void setCloudForwardEnabled(boolean enabled) {
        nativeSetCloudForwardEnabled(requireHandle(), enabled ? 1 : 0);
    }

    static void initializeForwarding(android.content.Context context) {
        nativeInitializeForwarding(context.getApplicationContext());
    }

    int forwardingWriteBudgetMs(long task) { return nativeForwardingWriteBudgetMs(requireHandle(), task); }

    long startForwarding(DecodedFrame frame) {
        if (!frame.forwardingOnly) throw new IllegalArgumentException("Not a forwarding frame.");
        return startForwarding(frame.messageType, frame.payload, frame.observationTicket);
    }

    long startForwarding(int messageType, byte[] payload, String observationTicket) {
        return nativeStartForwarding(requireHandle(), messageType, payload, observationTicket);
    }

    EncodedMessage nextForwarding(long task) {
        byte[] encoded = nativeNextForwarding(requireHandle(), task);
        if (encoded == null || encoded.length == 0) return null;
        if (encoded.length < 2) throw new IllegalStateException("Invalid forwarding response envelope.");
        try {
            int type = (encoded[0] & 0xff) | ((encoded[1] & 0xff) << 8);
            return new EncodedMessage(type, Arrays.copyOfRange(encoded, 2, encoded.length));
        } finally { Arrays.fill(encoded, (byte) 0); }
    }

    boolean forwardingFinished(long task) { return nativeForwardingFinished(requireHandle(), task) != 0; }
    void cancelForwarding(long task) { nativeCancelForwarding(requireHandle(), task); }
    void closeForwarding(long task) { nativeCloseForwarding(requireHandle(), task); }

    List<byte[]> encodeForwardingReports(long task, EncodedMessage message, boolean includeReportId, long nowMs) {
        int reportSize = includeReportId ? REPORT_SIZE : REPORT_SIZE - 1;
        byte[] encoded = nativeEncodeForwardingReports(requireHandle(), task, message.messageType, message.payload, includeReportId ? 1 : 0);
        try {
        if (encoded.length == 0 || encoded.length % reportSize != 0) throw new IllegalStateException("Invalid forwarding reports.");
        List<byte[]> reports = new ArrayList<>(encoded.length / reportSize);
        for (int offset = 0; offset < encoded.length; offset += reportSize) reports.add(Arrays.copyOfRange(encoded, offset, offset + reportSize));
        return reports;
        } finally { Arrays.fill(encoded, (byte) 0); }
    }

    boolean observationCurrent(String ticket) {
        return nativeObservationCurrent(requireHandle(), ticket) == 1;
    }

    synchronized void reset() { nativeReset(requireHandle()); }

    @Override
    public synchronized void close() {
        if (handle == 0) return;
        nativeDestroy(handle);
        handle = 0;
    }

    private synchronized long requireHandle() {
        if (handle == 0) throw new IllegalStateException("Native QC decoder is closed.");
        return handle;
    }

    private EncodedMessage one(String command, JSObject args) throws Exception {
        List<EncodedMessage> messages = commands(command, args);
        if (messages.size() != 1) throw new IllegalStateException("Native QC command did not produce exactly one message.");
        return messages.get(0);
    }

    private List<EncodedMessage> commands(String command, JSObject args) throws Exception {
        return encodedMessages(new JSONArray(nativeEncodeCommand(requireHandle(), command, args.toString())));
    }

    private static List<JSObject> objects(String json) throws Exception {
        JSArray values = new JSArray(json);
        List<JSObject> result = new ArrayList<>();
        for (int index = 0; index < values.length(); index++) {
            result.add(JSObject.fromJSONObject((JSONObject) values.get(index)));
        }
        return result;
    }

    private static long nonNegativeLong(JSONObject value, String key) throws Exception {
        long result = value.getLong(key);
        if (result < 0) throw new IllegalStateException("Native QC gateway " + key + " is invalid.");
        return result;
    }

    private static String nullableString(JSONObject value, String key) throws Exception {
        return value.isNull(key) ? null : value.getString(key);
    }

    private static long[] nonNegativeLongArray(JSONArray values) throws Exception {
        long[] result = new long[values.length()];
        for (int index = 0; index < result.length; index++) {
            result[index] = values.getLong(index);
            if (result[index] < 0) throw new IllegalStateException("Native QC gateway timing is invalid.");
        }
        return result;
    }

    private static List<EncodedMessage> encodedMessages(JSONArray values) throws Exception {
        List<EncodedMessage> result = new ArrayList<>(values.length());
        for (int index = 0; index < values.length(); index++) {
            JSONObject value = values.getJSONObject(index);
            int messageType = value.getInt("messageType");
            JSONArray payloadValues = value.getJSONArray("payload");
            byte[] payload = new byte[payloadValues.length()];
            for (int payloadIndex = 0; payloadIndex < payload.length; payloadIndex++) {
                int octet = payloadValues.getInt(payloadIndex);
                if (octet < 0 || octet > 255) throw new IllegalStateException("Native QC payload octet is invalid.");
                payload[payloadIndex] = (byte) octet;
            }
            result.add(new EncodedMessage(messageType, payload));
        }
        return result;
    }

    private static native long nativeCreate();
    private static native String nativeValidateBackupName(String name);
    private static native String nativeInspectBackupChecksum(byte[] utf8);
    private static native String nativeMergeExpectedState(String paramsJson, String expectedJson);
    private static native int nativeReportSize();
    private static native int nativeOutboundReportId();
    private static native int nativeInboundReportId();
    private static native String nativeEncodeCommand(long handle, String command, String argsJson);
    private static native String nativePlanGatewayWrite(long handle, String method, String argsJson, long startedMs);
    private static native String nativeBeginPresetWorkflow(long handle, String method, String argsJson, long nowMs);
    private static native String nativeAdvancePresetWorkflow(long handle, long preparation, long nowMs);
    private static native void nativeCancelPresetPreparation(long handle, long preparation);
    private static native String nativePlanGatewayRead(long handle, String method, String argsJson, String scope, long startedMs);
    private static native String nativeBeginGatewayReadScope(long handle, String method, String argsJson, String context, long startedMs);
    private static native String nativeBeginGatewayReadbackScope(long handle, String context, long startedMs, long timeoutMs);
    private static native long nativeGatewayReadScopeRemaining(long handle, String scope, long nowMs);
    private static native int nativeReadEpochCurrent(long handle, String sessionEpoch);
    private static native String nativeObserveGatewayRead(long handle, String ticket, String context, String projectionJson, byte[] payload, int expectedType, int actualType, long startedAtMs, long timeoutMs, long nowMs);
    private static native int nativeGatewayResponseDecision(
        String projectionJson, int expectedType, int actualType, byte[] payload, long startedAtMs, long timeoutMs, long nowMs);
    private static native int nativeRequestExpired(long startedAtMs, long timeoutMs, long nowMs);
    private static native String nativeBackupStarted(long handle, String operation, long nowMs);
    private static native String nativeBackupAdvance(long handle, String operation, long nowMs);
    private static native void nativeClaimBackupRecovery(long handle, String operation);
    private static native void nativeBackupCancelled(long handle, String operation);
    private static native String nativeConsumeBackupChunk(long handle, String operation, String observation, byte[] payload, String name, long nowMs);
    private static native String nativeHandshakeAttempt(long handle, long nowMs, String sessionId);
    private static native int nativeStartupConnected(long handle);
    private static native int nativeStartupActive(long handle);
    private static native String nativeLifecyclePoll(long handle, long nowMs);
    private static native String nativeLifecycleNextBatch(long handle, long nowMs, long unixTimeMs);
    private static native int nativeLifecycleBatchCurrent(long handle, String ticket);
    private static native String nativeLifecycleDispatched(long handle, String ticket, int accepted, long nowMs);
    private static native String nativeCatalogContext(long handle, String context);
    private static native long nativeGatewayVerificationStarted(
        long handle, String verificationJson, String contextJson, String policyMethod, long afterSequence,
        long startedAtMs, long timeoutMs);
    private static native String nativeGatewayVerificationAdvance(
        long handle, long verificationId, long observationSequence, long nowMs,
        String method, String verificationJson);
    private static native void nativeGatewayVerificationCancelled(long handle, long verificationId);
    private static native int nativeGatewayReadbackMatches(
        String method, String paramsJson, String responseJson);
    private static native long nativeGatewayReadbackRetryDelay(String method, int attempt);
    private static native String nativeComposeGlobalTempoSettings(String globalJson, String presetJson);
    private static native String nativeBindGatewayWritePreflight(
        long handle, String context, String method, String paramsJson, String responseJson, long nowMs);
    private static native byte[] nativeEncodeReports(
        long handle, int messageType, byte[] payload, int includeReportId, String dispatchTicket, long nowMs);
    private static native String nativePlanOperation(long handle, String sessionFence, long nowMs, long timeoutMs);
    private static native String nativePrepareDispatch(long handle, String operationTicket, long nowMs);
    private static native long nativeOperationRemaining(long handle, String operationTicket, long nowMs);
    private static native String nativePrepareDelivery(long handle, String dispatch, int messageType, int reportCount, long nowMs);
    private static native void nativeDeliveryEvent(long handle, String ticket, int event, int index, long nowMs);
    private static native int nativeObservationCurrent(long handle, String ticket);
    private static native long nativeCaptureReportEpoch(long handle, long connection);
    private static native byte[] nativePushReport(long handle, long connection, long reportEpoch, byte[] report, long receivedAtMs);
    private static native void nativeSetCloudForwardEnabled(long handle, int enabled);
    private static native void nativeInitializeForwarding(android.content.Context context);
    private static native int nativeForwardingWriteBudgetMs(long handle, long task);
    private static native long nativeStartForwarding(long handle, int messageType, byte[] payload, String observation);
    private static native byte[] nativeNextForwarding(long handle, long task);
    private static native int nativeForwardingFinished(long handle, long task);
    private static native void nativeCancelForwarding(long handle, long task);
    private static native void nativeCloseForwarding(long handle, long task);
    private static native byte[] nativeEncodeForwardingReports(long handle, long task, int messageType, byte[] payload, int includeReportId);
    private static native void nativeReset(long handle);
    private static native void nativeDestroy(long handle);
    private static native String nativeQueueModelRepo(long handle, String ticket, byte[] payload);
    private static native String nativeModelRepoReady(long handle);
    private static native String nativeParseNextModelRepo(long handle, long nowMs);
    private static native String nativeStatePublicationsReady(long handle);
    private static native String nativeTakeStatePublications(long handle);
    private static native String nativeAssertExpectedPreset(long handle, String params);
    private static native String nativeBlockDetails(long handle, int row, int column, String params);
    private static native String nativeLaneControlDetails(long handle, int row, String control, String params);
    private static native String nativeSnapshot(long handle);
    private static native String nativePreflightObservation(long handle, String method);
    private static native String nativePublishedSettings(long handle, int messageType);
    private static native String nativeCurrentPreset(long handle);
    private static native String nativeCurrentPresetObservation(long handle);
    private static native long nativeFullPresetRevision(long handle);
    private static native int nativeModelCount(long handle);
    private static native String nativeModelList(long handle);
    private static native String nativeRetainedCatalog(long handle, String context, String method, String key, long nowMs);
    private static native long nativeSessionOpened(long handle, long nowMs);

    private static native int nativeSessionConnected(long handle);
    private static native int nativeSessionSynchronized(long handle);
    private static native String nativeSuspension(long handle);
    private static native String nativeSuspensionRefusal(long handle);
    private static native String nativeTakeSuspensionResync(long handle);
    private static native int nativeSessionShouldKeepalive(long handle, long nowMs);
    private static native void nativeSessionKeepaliveSent(long handle, long nowMs);
    private static native void nativeSessionDisconnected(long handle, long nowMs);
    private static native long nativeSessionScheduleReconnect(long handle, long nowMs);
    private static native int nativeSessionReconnectDue(long handle, long nowMs);
    private static native void nativeSessionReconnectAttempted(long handle, long nowMs);
    private static native int nativeSessionTerminalReadFailed(long handle);
}
