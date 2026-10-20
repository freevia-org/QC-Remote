package org.freevia.qcremote;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.os.Build;
import android.os.IBinder;
import androidx.annotation.Nullable;
import androidx.core.app.NotificationCompat;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.WebSocket;
import okhttp3.WebSocketListener;
import org.json.JSONObject;

/** Foreground owner for the phone's outbound, authenticated relay connection. */
public final class QcRelayService extends Service {
    private static volatile QcRelayService runningInstance;
    static final String ACTION_START = "org.freevia.qcremote.relay.START";
    static final String ACTION_STOP = "org.freevia.qcremote.relay.STOP";
    static final String ACTION_STATUS = "org.freevia.qcremote.relay.STATUS";
    static final String EXTRA_STATE = "state";
    private static final String CHANNEL = "qc_relay";
    private static final int NOTIFICATION_ID = 7311;
    private static final int MAX_IN_FLIGHT_INVOKES = 8;
    private final ScheduledExecutorService scheduler = Executors.newSingleThreadScheduledExecutor();
    private final AtomicInteger generation = new AtomicInteger();
    private final QcRelayInvocationAdmission invocationAdmission =
        new QcRelayInvocationAdmission(MAX_IN_FLIGHT_INVOKES);
    private final QcRelayInvocationOwner<java.util.concurrent.CompletableFuture<JSONObject>> invocations =
        new QcRelayInvocationOwner<>();
    private OkHttpClient http;
    private WebSocket socket;
    private RelayCredentialStore.Snapshot socketIdentity;
    private ScheduledFuture<?> retry;
    private ScheduledFuture<?> readiness;
    private final RelayReadinessPublication readinessPublication = new RelayReadinessPublication();
    private int failures;
    private volatile RelayState state = RelayState.STOPPED;
    private final RelayFrameHandler relayFrameHandler = new RelayFrameHandler();

    @Override public void onCreate() {
        super.onCreate();
        // Persisted state describes the last connection, not this Service
        // instance. A newly created instance has no socket until onStartCommand
        // reaches connect(), even when the stored value says "connected".
        state = RelayState.STOPPED;
        runningInstance = this;
        http = new OkHttpClient.Builder()
            .pingInterval(GeneratedRelayProfile.PING_INTERVAL_MS, TimeUnit.MILLISECONDS).build();
        createChannel();
    }

    @Override public synchronized int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_STOP.equals(intent.getAction())) {
            stopRelay();
            stopSelf();
            return START_NOT_STICKY;
        }
        if (!new RelayCredentialStore(this).paired()) { updateState(RelayState.PAIRING_REQUIRED); stopSelf(); return START_NOT_STICKY; }
        // Android 14+ validates the connectedDevice foreground-service
        // prerequisite at startForeground(). Starting before the USB grant is
        // not a recoverable state: it throws SecurityException and kills the
        // whole app process. Stop cleanly and let the app restart us after its
        // USB session owns the attached QC.
        if (!QcUsbPlugin.relayForegroundServiceEligible()) {
            state = RelayState.STOPPED;
            sendBroadcast(new Intent(ACTION_STATUS).setPackage(getPackageName()).putExtra(EXTRA_STATE, state.wireValue()));
            stopSelf();
            return START_NOT_STICKY;
        }
        try {
            startForeground(NOTIFICATION_ID, notification("Connecting securely…"));
        } catch (RuntimeException error) {
            // Foreground-service eligibility can change between the check
            // above and this platform call. Fail this service start cleanly;
            // do not let Android's prerequisite exception kill the app.
            android.util.Log.w("QcRelayService", "Could not start the relay foreground service.", error);
            state = RelayState.STOPPED;
            sendBroadcast(new Intent(ACTION_STATUS).setPackage(getPackageName()).putExtra(EXTRA_STATE, state.wireValue()));
            stopSelf();
            return START_NOT_STICKY;
        }
        // START_STICKY may recreate this Service after process death with a
        // null intent. `state` is persisted for the UI, but the WebSocket is
        // process-local; an old "connected" value cannot prove a transport
        // exists in this Service instance.
        final RelayCredentialStore.Snapshot durable;
        try { durable=new RelayCredentialStore(this).snapshot(); }
        catch (Exception unconfirmed) { stopRelay(); stopSelf(); return START_NOT_STICKY; }
        if (!shouldConnectOnStart(state.wireValue(), socket != null, socketIdentity, durable)) return START_STICKY;
        generation.incrementAndGet();
        if (retry != null) retry.cancel(false);
        if (socket != null) { stopReadinessUpdates(socket); socket.close(1000,"pairing superseded"); }
        socket=null; socketIdentity=null;
        connect(generation.get());
        return START_STICKY;
    }

    static boolean shouldConnectOnStart(String state, boolean hasSocket) {
        boolean active = RelayState.parse(state).ownsActiveSocket();
        return !active || !hasSocket;
    }

    static boolean shouldConnectOnStart(String state,boolean hasSocket,RelayCredentialStore.Snapshot live,RelayCredentialStore.Snapshot durable) {
        return shouldConnectOnStart(state,hasSocket) || live == null || !RelayCredentialStore.sameIdentity(live,durable);
    }

    static String liveState() {
        QcRelayService service = runningInstance;
        return service == null ? null : service.state.wireValue();
    }

    static String stateForStatus(String liveState, String persistedState) {
        if (liveState != null) return liveState;
        // Never present a durable last-known active state as a current relay
        // connection when Android has no Service/WebSocket process to own it.
        if ("connected".equals(persistedState) || "connecting".equals(persistedState)
            || "reconnecting".equals(persistedState)) return "stopped";
        return persistedState;
    }

    private synchronized void connect(int expectedGeneration) {
        if (expectedGeneration != generation.get()) return;
        final int attemptGeneration = generation.incrementAndGet();
        RelayCredentialStore store = new RelayCredentialStore(this);
        final RelayCredentialStore.Snapshot held;
        try { held = store.snapshot(); }
        catch (Exception error) {
            // A keystore read failure is not evidence that the pairing was
            // revoked. Keep the credential and retry under the ordinary
            // bounded relay backoff rather than forcing the owner to pair again.
            android.util.Log.w("QcRelayService", "Could not read the relay credential; retaining pairing and retrying.", error);
            updateState(RelayState.RECONNECTING);
            reconnect(attemptGeneration);
            return;
        }
        String endpoint=held.endpoint;
        String deviceId=held.deviceId;
        String credential=held.credential;
        if (endpoint == null || credential == null || deviceId == null || deviceId.trim().isEmpty()) { updateState(RelayState.PAIRING_REQUIRED); stopSelf(); return; }
        // A pairing stored by an older build may name any HTTPS host. The
        // credential goes only to a relay this build permits.
        String permitted = QcRelayPlugin.normalizedEndpoint(endpoint);
        if (permitted == null) { updateState(RelayState.INVALID_ENDPOINT); stopSelf(); return; }
        String wsEndpoint = relaySocketUrl(permitted, deviceId);
        final String previousDeviceId;
        try { previousDeviceId=store.previousDeviceId(); }
        catch (Exception unavailable) { updateState(RelayState.STOPPED); return; }
        wsEndpoint = relaySocketUrl(permitted, deviceId, previousDeviceId);
        Request request = new Request.Builder().url(wsEndpoint)
            .header("Authorization", "Bearer " + credential)
            .header("Sec-WebSocket-Protocol", RelayProtocol.VERSION)
            .build();
        updateState(RelayState.CONNECTING);
        socketIdentity=held;
        socket = http.newWebSocket(request, new Listener(attemptGeneration,held,previousDeviceId));
    }

    /** The relay socket URL for a permitted HTTPS endpoint. The device id is
     * relay-issued, but it is still data: encoded, so it can only ever be the
     * value of the one query parameter and never add another. */
    static String relaySocketUrl(String endpoint, String deviceId) {
        String encodedId;
        try { encodedId = java.net.URLEncoder.encode(deviceId, "UTF-8").replace("+", "%20"); }
        catch (java.io.UnsupportedEncodingException impossible) { throw new IllegalStateException(impossible); }
        return endpoint.replaceFirst("^https://", "wss://").replaceAll("/$", "")
            + GeneratedRelayProfile.DEVICE_CONNECT_PATH + "?deviceId=" + encodedId;
    }

    static String relaySocketUrl(String endpoint, String deviceId, String previousDeviceId) {
        String url = relaySocketUrl(endpoint, deviceId);
        return previousDeviceId == null ? url
            : url + "&" + GeneratedRelayProfile.REPLACEMENT_ACK_QUERY_PARAMETER + "=1";
    }

    static boolean connectResponseMatches(RelayCredentialStore.Snapshot held,Response response, String previousDeviceId) {
        if (response == null) return false;
        String origin=QcRelayPlugin.normalizedEndpoint(held.endpoint);
        if (origin == null) return false;
        okhttp3.HttpUrl expected=new Request.Builder()
            .url(relaySocketUrl(origin,held.deviceId,previousDeviceId)).build().url();
        return expected.equals(response.request().url());
    }

    static boolean currentConnection(int expectedGeneration, int currentGeneration,
            RelayCredentialStore.Snapshot expected, RelayCredentialStore.Snapshot current) {
        return expectedGeneration == currentGeneration && RelayCredentialStore.sameIdentity(expected,current);
    }

    static boolean replacementAckMatches(JSONObject frame,String previousDeviceId) {
        return previousDeviceId != null && GeneratedRelayProfile.REPLACEMENT_ACK_FRAME_TYPE.equals(frame.opt("type"))
            && previousDeviceId.equals(frame.opt("previousDeviceId")) && Boolean.FALSE.equals(frame.opt("previousCredentialAccepted"));
    }

    /** OkHttp does not answer a server Close automatically. Retain rejection
     * evidence before an eventual transport failure loses the peer close code. */
    static void handlePeerClosing(WebSocket webSocket, int code, boolean current,
            Runnable stopReadiness, Runnable rejectPairing, Runnable reconnecting) {
        webSocket.close(code == 1005 ? 1000 : code, "relay closed");
        if (!current) return;
        stopReadiness.run();
        if (GeneratedRelayProfile.pairingRejectionClose(code)) rejectPairing.run();
        else reconnecting.run();
    }

    private final class Listener extends WebSocketListener {
        private final int expectedGeneration;
        private final RelayCredentialStore.Snapshot held;
        private final String previousDeviceId;
        Listener(int expectedGeneration,RelayCredentialStore.Snapshot held,String previousDeviceId) { this.expectedGeneration=expectedGeneration; this.held=held; this.previousDeviceId=previousDeviceId; }
        private boolean isCurrentListener() {
            synchronized (QcRelayService.this) {
                try { return currentConnection(expectedGeneration,generation.get(),held,new RelayCredentialStore(QcRelayService.this).snapshot()); }
                catch (Exception unavailable) { return false; }
            }
        }
        private boolean recordRejection() {
            synchronized (QcRelayService.this) {
                RelayCredentialStore store = new RelayCredentialStore(QcRelayService.this);
                try {
                    if (!currentConnection(expectedGeneration,generation.get(),held,store.snapshot())) return false;
                } catch (Exception unavailable) { return false; }
                if (!store.markRejected(held,true)) return false;
                generation.incrementAndGet(); updateState(RelayState.PAIRING_REQUIRED); stopSelf(); return true;
            }
        }

        @Override public void onOpen(WebSocket webSocket, Response response) {
            synchronized (QcRelayService.this) {
            if (expectedGeneration != generation.get()) { webSocket.close(1000, "superseded"); return; }
            if (!RelayProtocol.VERSION.equals(response.header("Sec-WebSocket-Protocol"))) { webSocket.close(1002,"unexpected relay protocol"); return; }
            if (!connectResponseMatches(held, response, previousDeviceId)) { webSocket.close(1000,"unexpected relay response origin"); return; }
            if (!new RelayCredentialStore(QcRelayService.this).markRejected(held,false)) {
                webSocket.close(1000,"superseded pairing"); return;
            }
            failures = 0;
            updateState(RelayState.CONNECTED);
            startReadinessUpdates(webSocket);
            }
        }

        /** The relay protocol is text-only. A binary message is refused and the
         * socket closed rather than silently dropped, as OkHttp's default does. */
        @Override public void onMessage(WebSocket webSocket, okio.ByteString bytes) {
            if (!isCurrentListener()) { webSocket.close(1000,"superseded"); return; }
            webSocket.close(1003, GeneratedRelayProfile.BINARY_UNSUPPORTED_TEXT);
        }

        /** OkHttp 4.12 offers no inbound message-size limit: RealWebSocket
         * reassembles every frame of a message into memory before it calls
         * here, and the upgraded stream is not reachable from an interceptor.
         * The limit below is therefore enforced at the first point this app
         * sees the message, before it is parsed or dispatched, and an oversized
         * message closes the socket (1009). The buffering itself is bounded
         * only by who can speak on this socket: the allowlisted relay,
         * authenticated over TLS (RelayEndpointPolicy). */
        @Override public void onMessage(WebSocket webSocket, String text) {
            if (utf8Length(text) > GeneratedRelayProfile.MAX_REQUEST_FRAME_BYTES) { webSocket.close(1009,GeneratedRelayProfile.MESSAGE_TOO_LARGE_TEXT); return; }
            if (!isCurrentListener()) { webSocket.close(1000,"superseded"); return; }
            try {
                JSONObject frame=new JSONObject(text);
                if (GeneratedRelayProfile.REPLACEMENT_ACK_FRAME_TYPE.equals(frame.opt("type"))) {
                    synchronized (QcRelayService.this) {
                        RelayCredentialStore store=new RelayCredentialStore(QcRelayService.this);
                        if (!currentConnection(expectedGeneration,generation.get(),held,store.snapshot())
                            || !replacementAckMatches(frame,previousDeviceId)) { webSocket.close(1002,"invalid replacement acknowledgement"); return; }
                        store.acknowledgeReplacement(held,frame.getString("previousDeviceId"));
                    }
                    return;
                }
            } catch (Exception malformed) { webSocket.close(1002,"invalid relay frame"); return; }
            RelayFrameHandler.Decision decision = relayFrameHandler.inspect(text,
                RelayProtocol::isAllowed,
                method -> RelayAccessPolicy.permits(QcRelayService.this, method));
            if (decision.kind == RelayFrameHandler.Kind.CLOSE) {
                webSocket.close(decision.closeCode, decision.closeReason);
                return;
            }
            if (decision.kind == RelayFrameHandler.Kind.ERROR) {
                sendError(webSocket, decision.id, decision.errorCode,
                    decision.errorMessage, decision.retryable);
                return;
            }
            // The initiating AI client owns action confirmation. The phone
            // host has already enforced pairing, access mode and the generated
            // method/parameter contract above; its device workflow retains
            // exact target and expected-name checks.
            dispatch(webSocket, decision.id, decision.method, decision.params,
                decision.expected, decision.deadlineMs);
        }

        private void dispatch(WebSocket webSocket, String id, String method,
                JSONObject params, JSONObject expected, long deadlineMs) {
            final java.util.concurrent.CompletableFuture<JSONObject> invocation;
            synchronized (QcRelayService.this) {
            // Linearize admission and ownership registration against stop/unpair.
            if (expectedGeneration != generation.get() || webSocket != socket) return;
            if (!invocationAdmission.tryAcquire()) {
                sendError(webSocket, id, "BUSY",
                    "The Android device host is at its bounded in-flight request limit.", true);
                return;
            }
            try { invocation = QcUsbPlugin.invokeFromRelay(method, params, expected); }
            catch (RuntimeException error) {
                invocationAdmission.release();
                sendError(webSocket, id, "DEVICE_ERROR", "The device request could not be admitted.");
                return;
            }
            invocations.track(invocation, () -> QcUsbPlugin.expireRelayInvocation(invocation));
            QcUsbPlugin.relayInvocationRetirement(invocation).whenComplete((ignored, failure) -> invocations.retire(invocation));
            }
            AtomicBoolean responseSent = new AtomicBoolean();
            final java.util.concurrent.ScheduledFuture<?> deadline;
            try {
                deadline = scheduler.schedule(() -> {
                    if (!responseSent.compareAndSet(false, true)) return;
                    QcUsbPlugin.expireRelayInvocation(invocation);
                    sendError(webSocket, id, "DEVICE_OUTCOME_UNKNOWN",
                        "The device operation exceeded its request deadline. Its outcome is unknown; do not retry it automatically.");
                }, deadlineMs, TimeUnit.MILLISECONDS);
            } catch (java.util.concurrent.RejectedExecutionException error) {
                // The operation has already been admitted. Keep its slot until
                // it really finishes, and report an unknown outcome instead
                // of encouraging a retry during service shutdown.
                QcUsbPlugin.expireRelayInvocation(invocation);
                QcUsbPlugin.relayInvocationRetirement(invocation)
                    .whenComplete((ignored, failure) -> invocationAdmission.release());
                sendError(webSocket, id, "DEVICE_OUTCOME_UNKNOWN",
                    "The relay stopped tracking this device operation. Its outcome is unknown; do not retry it automatically.");
                return;
            }
            invocation
                .whenComplete((result, error) -> {
                    // A protocol result may time out while its runnable is
                    // still queued behind another USB operation. Keep the
                    // admission slot until both the result and all owned work
                    // have retired, so repeated timeouts cannot grow the queue.
                    QcUsbPlugin.relayInvocationRetirement(invocation)
                        .whenComplete((ignored, retirementFailure) -> invocationAdmission.release());
                    if (!responseSent.compareAndSet(false, true)) return;
                    deadline.cancel(false);
                    JSONObject reply;
                    try {
                        Throwable cause = unwrapCompletion(error);
                        if (cause != null) android.util.Log.w(
                            "QcRelayService", "QC relay invocation failed for " + method + ": "
                                + cause.getClass().getSimpleName() + ": " + cause.getMessage());
                        reply = error == null
                            ? validatedResult(id, method, result)
                            : cause instanceof RelayException
                                ? RelayProtocol.error(id, (RelayException) cause)
                                : RelayProtocol.error(id, QcSuspensionGate.code(cause, "DEVICE_ERROR"),
                                cause == null || cause.getMessage() == null
                                    ? "The device operation failed." : cause.getMessage(),
                                cause instanceof RelayException
                                    && ((RelayException) cause).retryable());
                    } catch (Exception validationError) {
                        android.util.Log.e("QcRelayService",
                            "QC relay result validation failed for " + method + ": "
                                + validationError.getMessage(), validationError);
                        try {
                            reply = RelayProtocol.error(id, "MALFORMED_RESPONSE",
                                "The device returned a malformed result.", false);
                        } catch (Exception encodingError) {
                            android.util.Log.e("QcRelayService", "Could not encode relay validation error.", encodingError);
                            return;
                        }
                    }
                    try {
                        // WebSocket frames are ordered. Publish the post-operation
                        // USB state before the result so reset/reconnect callers
                        // cannot race the next local readiness observation.
                        sendReadiness(webSocket);
                        sendResult(webSocket, id, reply);
                    } catch (Exception sendError) {
                        android.util.Log.w("QcRelayService", "Could not send QC relay result for " + method + ".", sendError);
                    }
                });
        }

        private Throwable unwrapCompletion(Throwable error) {
            Throwable cause = error;
            while (cause instanceof java.util.concurrent.CompletionException && cause.getCause() != null) {
                cause = cause.getCause();
            }
            return cause;
        }

        private JSONObject validatedResult(String id, String method, JSONObject result) throws Exception {
            GeneratedGatewayMethods.validateResult(method, result);
            return RelayProtocol.result(id, result);
        }

        @Override public void onClosing(WebSocket webSocket, int code, String reason) {
            synchronized (QcRelayService.this) {
                handlePeerClosing(webSocket,code,isCurrentListener(),
                    () -> stopReadinessUpdates(webSocket),
                    () -> recordRejection(),
                    () -> updateState(RelayState.RECONNECTING));
            }
        }

        @Override public void onClosed(WebSocket webSocket, int code, String reason) {
            android.util.Log.w("QcRelayService", "Relay WebSocket closed (code " + code + ").");
            stopReadinessUpdates(webSocket);
            if (GeneratedRelayProfile.pairingRejectionClose(code) && recordRejection()) return;
            reconnect(expectedGeneration);
        }
        @Override public void onFailure(WebSocket webSocket, Throwable error, Response response) {
            android.util.Log.w("QcRelayService", "Relay WebSocket failed"
                + (response == null ? "." : " with HTTP " + response.code() + "."), error);
            stopReadinessUpdates(webSocket);
            if (response != null && connectResponseMatches(held,response,previousDeviceId)
                && GeneratedRelayProfile.pairingRejectionHttp(response.code()) && recordRejection()) return;
            reconnect(expectedGeneration);
        }
    }

    private synchronized void startReadinessUpdates(WebSocket webSocket) {
        if (readiness != null) readiness.cancel(false);
        readinessPublication.reset();
        sendReadiness(webSocket);
        readiness = scheduler.scheduleWithFixedDelay(
            () -> sendReadiness(webSocket), GeneratedRelayProfile.READINESS_INTERVAL_MS,
            GeneratedRelayProfile.READINESS_INTERVAL_MS, TimeUnit.MILLISECONDS);
    }

    private synchronized void stopReadinessUpdates(WebSocket webSocket) {
        if (webSocket != socket) return;
        if (readiness != null) readiness.cancel(false);
        readiness = null;
        readinessPublication.reset();
    }

    private synchronized void sendReadiness(WebSocket webSocket) {
        if (webSocket != socket || state != RelayState.CONNECTED) return;
        try {
            boolean ready = QcUsbPlugin.relaySessionAvailable();
            // Continue observing USB locally, but publish only actual changes.
            // OkHttp's protocol pings keep the idle socket alive without DO work.
            String frame = new JSONObject().put("type", "ready").put("protocol", RelayProtocol.VERSION)
                .put("gatewayApiVersion", GeneratedGatewayMethods.API_VERSION)
                .put("usbConnected", ready).toString();
            readinessPublication.publish(ready, () -> webSocket.send(frame));
        } catch (Exception ignored) {}
    }

    private void sendResult(WebSocket webSocket, String id, JSONObject reply) throws Exception {
        String text = reply.toString();
        if (utf8Length(text) > GeneratedRelayProfile.MAX_RESULT_FRAME_BYTES) {
            text = RelayProtocol.error(id, "RESULT_TOO_LARGE",
                "The device result exceeds the relay frame limit.", false).toString();
        }
        webSocket.send(text);
    }

    private void sendError(WebSocket webSocket, String id, String code, String message) {
        sendError(webSocket, id, code, message, false);
    }

    private void sendError(WebSocket webSocket, String id, String code, String message, boolean retryable) {
        try { webSocket.send(RelayProtocol.error(id, code, message, retryable).toString()); }
        catch (Exception ignored) {}
    }

    static long requestDeadlineMs(JSONObject request) {
        return RelayFrameHandler.requestDeadlineMs(request);
    }

    static int utf8Length(String value) {
        return value.getBytes(java.nio.charset.StandardCharsets.UTF_8).length;
    }

    private synchronized void reconnect(int failedGeneration) {
        if (failedGeneration != generation.get()) return;
        updateState(RelayState.RECONNECTING);
        failures = Math.min(failures + 1, GeneratedRelayProfile.MAXIMUM_FAILURE_COUNT);
        long base = Math.min(GeneratedRelayProfile.MAXIMUM_BACKOFF_SECONDS,
            1L << Math.min(failures, GeneratedRelayProfile.MAXIMUM_BACKOFF_EXPONENT));
        long jitterMs = (long) (Math.random() * GeneratedRelayProfile.BACKOFF_JITTER_MS);
        if (retry != null) retry.cancel(false);
        retry = scheduler.schedule(() -> connect(failedGeneration), base * 1000 + jitterMs, TimeUnit.MILLISECONDS);
    }

    private synchronized void stopRelay() {
        generation.incrementAndGet();
        // A stopped socket must not leave queued writes authorized after unpair.
        // Already-admitted writes expire through the same unknown-outcome and
        // quarantine path as deadlines; their owned slots wait for retirement.
        invocations.expireAll();
        if (retry != null) retry.cancel(false);
        if (readiness != null) readiness.cancel(false);
        readiness = null;
        readinessPublication.reset();
        if (socket != null) socket.close(1000, "stopped");
        socket = null;
        socketIdentity = null;
        updateState(RelayState.STOPPED);
        stopForeground(STOP_FOREGROUND_REMOVE);
    }

    private void updateState(RelayState next) {
        state = next;
        String value = next.wireValue();
        new RelayCredentialStore(this).setRelayState(value);
        ((NotificationManager) getSystemService(NOTIFICATION_SERVICE)).notify(NOTIFICATION_ID,
            notification(next == RelayState.CONNECTED ? "Ready for remote QC control" : value.replace('_', ' ')));
        sendBroadcast(new Intent(ACTION_STATUS).setPackage(getPackageName()).putExtra(EXTRA_STATE, value));
    }

    private Notification notification(String detail) {
        return new NotificationCompat.Builder(this, CHANNEL).setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(getString(R.string.relay_notification_title)).setContentText(detail).setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE).setPriority(NotificationCompat.PRIORITY_LOW).build();
    }

    private void createChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return;
        NotificationChannel channel = new NotificationChannel(CHANNEL, "QC remote connection", NotificationManager.IMPORTANCE_LOW);
        channel.setDescription("Keeps the secure Quad Cortex relay connected while USB is attached.");
        ((NotificationManager) getSystemService(NOTIFICATION_SERVICE)).createNotificationChannel(channel);
    }

    @Override public void onDestroy() {
        stopRelay();
        if (runningInstance == this) runningInstance = null;
        scheduler.shutdownNow();
        http.dispatcher().executorService().shutdown();
        super.onDestroy();
    }
    @Nullable @Override public IBinder onBind(Intent intent) { return null; }
}
