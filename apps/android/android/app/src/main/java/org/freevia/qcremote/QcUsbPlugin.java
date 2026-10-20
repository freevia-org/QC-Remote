package org.freevia.qcremote;

import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.hardware.usb.UsbConstants;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbDeviceConnection;
import android.hardware.usb.UsbEndpoint;
import android.hardware.usb.UsbInterface;
import android.hardware.usb.UsbManager;
import android.hardware.usb.UsbRequest;
import android.os.Build;
import androidx.annotation.RequiresApi;
import androidx.core.content.ContextCompat;
import com.getcapacitor.JSArray;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.getcapacitor.PluginMethod;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.ArrayDeque;
import java.util.Deque;
import java.util.List;
import java.util.UUID;
import java.io.File;
import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;

@CapacitorPlugin(name = "QcUsb")
public class QcUsbPlugin extends Plugin {
    private static long monotonicMillis() { return System.nanoTime() / 1_000_000L; }

    /** How long a validated session may deliver nothing before it is treated
     * as dead. A connected QC sends state continuously, so no frame at all in
     * this window is not slow, it is silent. Comfortably inside the ready wait
     * so recovery starts before anything else gives up. */
    private static final long SILENT_SESSION_TIMEOUT_MS = 10_000L;
    private static final int HID_WRITE_TIMEOUT_MS = 250;
    private static final int MIDI_WRITE_TIMEOUT_MS = 250;
    private static final int MAINTENANCE_POLL_MS = 1000;
    private static final int HID_INPUT_REQUEST_DEPTH = 32;
    private static final String USB_PERMISSION = "org.freevia.qcremote.USB_PERMISSION";

    private UsbManager manager;
    private UsbDevice device;
    private UsbDeviceConnection connection;
    private volatile UsbDeviceConnection lastClosedController;
    private UsbInterface hidInterface;
    private UsbEndpoint inputEndpoint;
    private volatile UsbRequest[] activeInputRequests;
    private UsbDeviceConnection midiConnection;
    private UsbInterface midiInterface;
    private UsbEndpoint midiOutputEndpoint;
    private volatile PluginCall pendingConnect;
    // HID reads, HID writes, and performance MIDI each have an independent
    // lane. A footswitch must never queue behind preset synchronization or a
    // multi-report parameter write.
    private final ExecutorService readerIo = Executors.newSingleThreadExecutor();
    private final ExecutorService commandIo = Executors.newSingleThreadExecutor();
    private final ExecutorService midiIo = Executors.newSingleThreadExecutor();
    private final ExecutorService metadataIo = new java.util.concurrent.ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, new java.util.concurrent.ArrayBlockingQueue<>(1));
    private final AtomicBoolean metadataDrainScheduled = new AtomicBoolean(false);
    private final ExecutorService statePublicationIo = new java.util.concurrent.ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, new java.util.concurrent.ArrayBlockingQueue<>(1));
    private final QcStatePublicationDrain statePublicationDrain = new QcStatePublicationDrain(
        statePublicationIo, this::drainStatePublications, this::statePublicationsReady, this::statePublicationFailed);
    private final ExecutorService backupFileIo = new java.util.concurrent.ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, new java.util.concurrent.ArrayBlockingQueue<>(1));
    private final ExecutorService lifecycleTeardown = Executors.newSingleThreadExecutor();
    // Host resource bound, not a protocol constant. A stalled
    // image decoder must neither block USB reception nor retain unlimited frames.
    private final ExecutorService readResponseIo = new java.util.concurrent.ThreadPoolExecutor(
        1, 1, 0L, TimeUnit.MILLISECONDS, new java.util.concurrent.ArrayBlockingQueue<>(64));
    private final ScheduledExecutorService keepalive = Executors.newSingleThreadScheduledExecutor();
    private volatile boolean reading;
    private volatile CountDownLatch resetReply;
    private volatile String currentSetlist;
    private volatile int currentPosition = -1;
    private final QcUsbDiagnostics usbDiagnostics = new QcUsbDiagnostics();
    private volatile long nativeConnectionToken;
    private volatile long decodeErrors;
    private volatile long expectedWriteStalls;
    private volatile long terminalHidWriteFailures;
    /** Set once a mutation reached the wire but its outcome could not be
     * established - the Android analog of the desktop broker's
     * `DeviceSession.writes_quarantined` (services/device-broker/src/worker.rs,
     * `quarantine_writes`). Reads keep working; only a fresh connect (the full
     * open+handshake below, never a no-op reconnect onto an already-ready
     * session) clears it, mirroring the desktop broker only clearing on a
     * session that re-synchronizes from a real reconnect/reset rather than on
     * the next successful write. */
    private final QcRelayAdmissionGuard relayAdmissionGuard = new QcRelayAdmissionGuard();
    private volatile int lastMessageType = -1;

    /** The QC message type for a settings family it publishes rather than answers, else -1. */
    private static int publishedSettingsType(String readMethod) {
        if ("device.generalSettings".equals(readMethod)) return QcUsbProfile.MESSAGE_TYPE_GENERAL_SETTINGS;
        if ("device.ioSettings".equals(readMethod)) return QcUsbProfile.MESSAGE_TYPE_IO_SETTINGS;
        return -1;
    }
    private volatile long connectedAt;
    private volatile String lastError;
    private volatile long readAttempts;
    private volatile long negativeReads;
    private volatile int selectedInterfaceId = -1;
    private volatile int selectedInputEndpointAddress = -1;
    private volatile int selectedInputMaxPacketSize;
    private volatile boolean includeReportId = true;
    private volatile long commandNotBeforeMs;
    private volatile boolean connecting;
    private volatile boolean transportClosing;
    /** When `connecting` was raised, so a latched flag can be told from a live
     * attempt. A connect cannot legitimately outlive the ready wait, so one
     * that has is stale by definition rather than by guess. */
    private volatile long connectingSinceMs;
    private volatile CompletableFuture<org.json.JSONObject> reconnectInFlight;
    private final QcPersistentMutationGate persistentMutationGate =
        new QcPersistentMutationGate();
    private volatile long lastMidiCommandAt;
    private volatile long lastMidiQueueDelayMs;
    private volatile long maxMidiQueueDelayMs;
    private volatile long lastStateAt;
    private volatile long lastHidWriteDurationMs;
    private volatile long maxHidWriteDurationMs;
    private volatile int lastHidWriteResult;
    private volatile boolean lastHidWriteIncludedReportId;
    private volatile String lastGatewayReadMismatch;
    private volatile boolean readerWaiting;
    private volatile long readerExitedAt;
    private volatile long readerTerminatedGeneration = -1;
    private volatile String lastReaderError;
    private final AtomicBoolean lifecycleDrainScheduled = new AtomicBoolean();
    private final AtomicBoolean lifecycleDrainRequested = new AtomicBoolean();
    // Only the command writer owns these publication snapshots.
    private boolean lifecyclePublishedConnected;
    private boolean lifecyclePublishedSynchronized;
    // RemoteControl screenshot replies carry no request ID. Keep the entire
    // reconnect/read/recovery lifecycle single-flight so one type-72 reply can
    // never complete two callers or be mistaken for an older capture.
    private final QcCaptureScreenGate captureScreenGate = new QcCaptureScreenGate();
    private final Object currentPresetRefreshLock = new Object();
    private CompletableFuture<org.json.JSONObject> currentPresetGridRefresh;
    private final AtomicLong connectionGeneration = new AtomicLong();
    // A recovery timer belongs to one operator connection intent. Explicit
    // disconnect/reconnect must not let an older timer re-enable recovery.
    private final AtomicLong automaticReconnectGeneration = new AtomicLong();
    private final QcPendingOperations pendingOperations = new QcPendingOperations();
    private final QcNativeStateDecoder stateDecoder = new QcNativeStateDecoder();
    private static final String INTERNET_FORWARDING_SETTING = "internetForwarding";
    private volatile boolean internetForwardingEnabled;
    private final Object internetForwardingSettingLock = new Object();
    private final QcInternetForwarder internetForwarder = new QcInternetForwarder(
        new QcInternetForwarder.Engine() {
            public long start(int type, byte[] payload, String observation) {
                return stateDecoder.startForwarding(type, payload, observation);
            }
            public QcNativeStateDecoder.EncodedMessage next(long task) throws Exception {
                return stateDecoder.nextForwarding(task);
            }
            public boolean finished(long task) { return stateDecoder.forwardingFinished(task); }
            public void cancel(long task) { stateDecoder.cancelForwarding(task); }
            public void release(long task) { stateDecoder.closeForwarding(task); }
        }, this::writeForwardingMessage);
    private static volatile QcUsbPlugin relaySession;
    private final QcDemoProvider demoProvider = new QcDemoProvider(
        QcUsbPlugin::relaySessionAvailable, connectionGeneration::get, this::currentBrokerTarget);
    private final QcBrokerTarget.Registry brokerTargets = new QcBrokerTarget.Registry();
    private static final ThreadLocal<RelayDeadline> CURRENT_RELAY_DEADLINE = new ThreadLocal<>();
    private static final ConcurrentHashMap<CompletableFuture<org.json.JSONObject>, RelayDeadline> RELAY_DEADLINES = new ConcurrentHashMap<>();

    private volatile int currentMasterVolume = -1;
    private volatile long currentMasterVolumeObservedAt = 0;
    private final Object stateEventLock = new Object();
    private final QcStateEventJournal<JSObject> stateEventLog = new QcStateEventJournal<>();
    private final String stateStreamId = java.util.UUID.randomUUID().toString();
    private long nextStateSequence = 1;
    private final QcSettingsEchoEvidence settingsEchoEvidence = new QcSettingsEchoEvidence();
    private volatile JSObject latestTempoClock;
    private volatile QcPendingOperations.Entry<PendingBackup> pendingBackup;
    private volatile QcPendingOperations.Entry<PendingReady> pendingReady;
    private QcUsbFlightRecorder flight;

    private final BroadcastReceiver permissionReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (!USB_PERMISSION.equals(intent.getAction())) return;
            UsbDevice grantedDevice = QcUsbDevicePolicy.deviceExtra(intent);
            boolean granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false);
            if (flight != null) flight.event(granted ? "usb-permission-granted" : "usb-permission-denied");
            PluginCall call = pendingConnect;
            pendingConnect = null;
            if (call == null) return;
            if (!granted || grantedDevice == null) {
                call.reject("USB permission was denied.", "USB_PERMISSION_DENIED");
                return;
            }
            openAndHandshake(grantedDevice, call);
        }
    };

    private final BroadcastReceiver deviceReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            UsbDevice changed = QcUsbDevicePolicy.deviceExtra(intent);
            if (changed == null || !QcUsbDevicePolicy.isQuadCortex(changed)) return;
            JSObject status = new JSObject();
            if (UsbManager.ACTION_USB_DEVICE_DETACHED.equals(intent.getAction())) {
                if (flight != null) flight.event("usb-detached");
                rejectPendingConnect("The Quad Cortex was unplugged before USB permission was answered.");
                if (device != null && device.getDeviceId() == changed.getDeviceId()) closeConnection();
                // A connectedDevice foreground service is only valid while the
                // app owns an attached device. Keep pairing credentials, but
                // stop the relay until USB permission is available again.
                getContext().stopService(new Intent(getContext(), QcRelayService.class));
                status.put("state", "disconnected");
            } else {
                if (flight != null) flight.event("usb-attached");
                status.put("state", "available");
                status.put("name", changed.getProductName() == null ? getContext().getString(R.string.device_name) : changed.getProductName());
            }
            notifyListeners("qcConnection", status, true);
        }
    };

    @Override
    public void load() {
        stateDecoder.initializeForwarding(getContext().getApplicationContext());
        internetForwardingEnabled = getContext().getSharedPreferences("qc-usb", Context.MODE_PRIVATE)
            .getBoolean(INTERNET_FORWARDING_SETTING, true);
        stateDecoder.setCloudForwardEnabled(internetForwardingEnabled);
        internetForwarder.setEnabled(internetForwardingEnabled);
        flight = new QcUsbFlightRecorder(getContext());
        flight.event("plugin-loaded");
        relaySession = this;
        manager = (UsbManager) getContext().getSystemService(Context.USB_SERVICE);
        IntentFilter filter = new IntentFilter(USB_PERMISSION);
        ContextCompat.registerReceiver(
            getContext(), permissionReceiver, filter, ContextCompat.RECEIVER_NOT_EXPORTED
        );
        IntentFilter deviceFilter = new IntentFilter();
        deviceFilter.addAction(UsbManager.ACTION_USB_DEVICE_ATTACHED);
        deviceFilter.addAction(UsbManager.ACTION_USB_DEVICE_DETACHED);
        ContextCompat.registerReceiver(
            getContext(), deviceReceiver, deviceFilter, ContextCompat.RECEIVER_NOT_EXPORTED
        );
        keepalive.scheduleWithFixedDelay(() -> {
            long now = monotonicMillis();
            requestLifecycleDrain();
            boolean backupActive = pendingBackup != null;
            if (!isReady() || backupActive
                || !stateDecoder.sessionShouldKeepalive(monotonicMillis())) return;
            // Keepalives share the serialized writer and retain their dedicated
            // cadence even while an ordinary correlated request is pending.
            commandIo.execute(() -> {
                boolean currentBackupActive = pendingBackup != null;
                if (!isReady() || currentBackupActive
                    || !stateDecoder.sessionShouldKeepalive(monotonicMillis())) return;
                try {
                    // The shared runtime uses the QC's dedicated KeepAlive on
                    // both platforms. A Version read can prove a reply, but it
                    // does not keep CorOS's push/session service alive.
                    writeMessage(stateDecoder.keepaliveCommand());
                    stateDecoder.sessionKeepaliveSent(monotonicMillis());
                } catch (Exception ignored) {}
            });
        }, MAINTENANCE_POLL_MS, MAINTENANCE_POLL_MS, TimeUnit.MILLISECONDS);
    }

    static boolean relaySessionAvailable() {
        QcUsbPlugin session = relaySession;
        return session != null && session.isReady()
            && session.stateDecoder.sessionSynchronized();
    }

    static boolean relayDemoAvailable() {
        QcUsbPlugin session = relaySession;
        return session != null && session.demoProvider.available();
    }

    static boolean relayProviderAvailable() {
        return relaySessionAvailable() || relayDemoAvailable();
    }

    static QcBrokerTarget relayBrokerTarget() {
        QcUsbPlugin session = relaySession;
        return session == null ? null : session.currentBrokerTarget();
    }

    private QcBrokerTarget currentBrokerTarget() {
        if (relaySessionAvailable()) return brokerTargets.select("physical", stateStreamId, connectionGeneration.get());
        if (demoProvider.available()) {
            QcDemoProvider.Registration registration = demoProvider.registration();
            if (registration != null) return brokerTargets.select("demo", registration.id, registration.generation);
        }
        return brokerTargets.select(null, null, 0);
    }

    boolean matchesBrokerTarget(QcBrokerTarget expected) { return expected.same(currentBrokerTarget()); }
    void assertBrokerTarget(QcBrokerTarget expected) { expected.assertCurrent(currentBrokerTarget()); }

    private org.json.JSONObject targetMetadata(String method, org.json.JSONObject result) throws Exception {
        return projectBrokerTarget(method, result, currentBrokerTarget());
    }

    static org.json.JSONObject projectBrokerTarget(String method, org.json.JSONObject result, QcBrokerTarget target) throws Exception {
        if ("system.status".equals(method) || "device.identity".equals(method)
                || "device.reconnect".equals(method) || "device.resetSession".equals(method)
                || "device.disconnect".equals(method)) {
            if (target != null) result.put("brokerTarget", target.json());
            else result.remove("brokerTarget");
            if ("system.status".equals(method)) {
                result.put("gatewayApiVersion", GeneratedGatewayMethods.API_VERSION);
                org.json.JSONArray capabilities = result.optJSONArray("capabilities");
                if (capabilities == null) { capabilities = capabilitiesJson(); result.put("capabilities", capabilities); }
                if (capabilities != null) {
                    boolean found = false;
                    for (int index = 0; index < capabilities.length(); index++)
                        if ("expectedBrokerTarget".equals(capabilities.optString(index))) found = true;
                    if (!found) capabilities.put("expectedBrokerTarget");
                }
                if (target != null && "demo".equals(target.source)) {
                    result.put("connected", true).put("synchronized", true)
                        .put("transport", "android-demo-relay");
                }
            }
        }
        return result;
    }

    @PluginMethod public void publishDemoProvider(PluginCall call) {
        String session = call.getString("sessionId");
        if (session != null && (session.isEmpty() || session.length() > 128)) {
            call.reject("The demo session identity is invalid.", "INVALID_PARAMS");
            return;
        }
        long generation = demoGeneration(call);
        if (generation < 0) {
            call.reject("The demo generation is invalid.", "INVALID_PARAMS");
            return;
        }
        demoProvider.publish(session, generation);
        startForegroundDemoRelay();
        call.resolve();
    }

    @PluginMethod public void admitDemoRequest(PluginCall call) {
        QcBrokerTarget target = null;
        if (call.getData().has("brokerTarget")) {
            try { target = QcBrokerTarget.parse(call.getData().opt("brokerTarget")); }
            catch (IllegalArgumentException malformed) { call.resolve(new JSObject().put("admitted", false)); return; }
        }
        boolean admitted = demoProvider.admit(call.getString("id", ""), call.getString("sessionId", ""),
            demoGeneration(call), target);
        call.resolve(new JSObject().put("admitted", admitted));
    }

    @PluginMethod public void completeDemoRequest(PluginCall call) {
        JSObject result = call.getObject("result");
        if (result != null) result.remove("brokerTargetTransition");
        String method = demoProvider.pendingMethod(call.getString("id", ""));
        if (result != null && method != null) {
            try { targetMetadata(method, result); }
            catch (Exception malformed) { result = null; }
        }
        demoProvider.complete(call.getString("id", ""), call.getString("sessionId", ""),
            demoGeneration(call), result, call.getObject("error"));
        call.resolve();
    }

    private static long demoGeneration(PluginCall call) {
        // Capacitor getLong rejects small JSON integers (Integer), including 1.
        try { return QcBrokerTarget.generation(call.getData(), 0); }
        catch (IllegalArgumentException malformed) { return -1; }
    }

    /** Android granted this app the attached device through its own
     * confirmation dialog, which arrives as an activity launch rather than as
     * a permission broadcast. Republishing availability lets the ordinary
     * connect path run again and find the permission already held, instead of
     * asking a second time for the grant the person just gave. */
    static void systemGrantedAttachment() {
        QcUsbPlugin session = relaySession;
        if (session == null || session.manager == null) return;
        UsbDevice candidate = session.findQuadCortex();
        if (candidate == null || !session.manager.hasPermission(candidate)) return;
        if (session.flight != null) session.flight.event("usb-system-grant");
        // The attachment confirmation belongs to Android's activity-launch
        // flow, not to our private USB_PERMISSION broadcast. If the web app
        // asked to connect while that system dialog was open, finish that
        // exact request here; merely publishing "available" leaves its
        // promise pending forever and the UI eventually falls back to Demo.
        PluginCall waiting = session.pendingConnect;
        session.pendingConnect = null;
        if (waiting != null) {
            session.openAndHandshake(candidate, waiting);
            return;
        }
        JSObject status = new JSObject();
        status.put("state", "available");
        status.put("name", candidate.getProductName() == null
            ? session.getContext().getString(R.string.device_name) : candidate.getProductName());
        session.notifyListeners("qcConnection", status, true);
    }

    static boolean relayForegroundServiceEligible() {
        QcUsbPlugin session = relaySession;
        if (session == null || session.manager == null) return false;
        UsbDevice candidate = session.findQuadCortex();
        return candidate != null && session.manager.hasPermission(candidate);
    }

    static CompletableFuture<org.json.JSONObject> invokeFromRelay(String method, org.json.JSONObject params, org.json.JSONObject expected) {
        return invokeFromRelay(method, params, expected, null);
    }

    static CompletableFuture<org.json.JSONObject> invokeFromRelay(String method, org.json.JSONObject params,
            org.json.JSONObject expected, QcBrokerTarget expectedTarget) {
        QcUsbPlugin session = relaySession;
        if (session == null) return failedRelay("NOT_CONNECTED", "QC Remote is not running with a Quad Cortex session.");
        QcBrokerTarget captured = session.currentBrokerTarget();
        if (expectedTarget != null && !expectedTarget.same(captured))
            return failedRelay("TARGET_SUPERSEDED", "The requested broker target is no longer active.");
        if (captured != null && "demo".equals(captured.source)) {
            return session.demoProvider.invoke(method,
                params == null ? new org.json.JSONObject() : params,
                expected == null ? new org.json.JSONObject() : expected,
                request -> session.getActivity().runOnUiThread(() -> {
                    if (!session.demoProvider.canDeliver(request.optString("id"))) return;
                    try {
                        session.notifyListeners("qcDemoRequest", JSObject.fromJSONObject(request), false);
                    } catch (Exception malformed) {
                        session.demoProvider.complete(request.optString("id"), request.optString("sessionId"),
                            request.optLong("generation"), null, null);
                    }
                }));
        }
        return session.invokePhysicalGateway(method, params, expected, captured);
    }

    private CompletableFuture<org.json.JSONObject> invokePhysicalGateway(String method, org.json.JSONObject params,
            org.json.JSONObject expected, QcBrokerTarget captured) {
        QcUsbPlugin session = this;
        if (captured == null && !"system.status".equals(method) && !"device.reconnect".equals(method))
            return failedRelay("NOT_CONNECTED", "No usable provider was available when this request arrived.");
        RelayDeadline deadline = new RelayDeadline(session, captured);
        boolean lifecycle = "device.reconnect".equals(method) || "device.resetSession".equals(method)
            || "device.disconnect".equals(method);
        UsbDeviceConnection originalController = lifecycle && captured != null ? session.connection : null;
        String originalPrivateKey = lifecycle && captured != null ? session.privateControllerKey() : null;
        CompletableFuture<org.json.JSONObject> invocation = new CompletableFuture<>();
        RELAY_DEADLINES.put(invocation, deadline);
        deadline.retirement().whenComplete((ignored, error) -> RELAY_DEADLINES.remove(invocation, deadline));
        CompletableFuture<org.json.JSONObject> operation;
        CURRENT_RELAY_DEADLINE.set(deadline);
        try {
            operation = session.relayInvoke(method, params == null ? new org.json.JSONObject() : params,
                expected == null ? new org.json.JSONObject() : expected);
        } catch (RuntimeException error) {
            operation = new CompletableFuture<>();
            operation.completeExceptionally(error);
        } finally { CURRENT_RELAY_DEADLINE.remove(); }
        deadline.attachOperation(operation);
        operation.whenComplete((result, error) -> {
            try {
                if (error != null) invocation.completeExceptionally(error);
                else {
                    QcBrokerTarget current = session.currentBrokerTarget();
                    result.remove("brokerTargetTransition");
                    if (captured != null && !captured.same(current)) {
                        if (!lifecycle) throw new RelayException("TARGET_SUPERSEDED", "The broker target changed while the request was running.");
                        String newPrivateKey = "device.disconnect".equals(method) ? null : session.privateControllerKey();
                        QcBrokerTarget afterIdentityRead = session.currentBrokerTarget();
                        boolean stable = current == null ? afterIdentityRead == null : current.same(afterIdentityRead);
                        org.json.JSONObject transition = stable ? QcBrokerTargetTransition.verified(method, captured, current,
                            originalPrivateKey, newPrivateKey, originalController != null
                                && session.lastClosedController == originalController && session.connection != originalController) : null;
                        if (transition == null) throw new RelayException("DEVICE_OUTCOME_UNKNOWN",
                            "The lifecycle operation completed, but the resulting controller could not be verified. Do not retry automatically.");
                        result.put("brokerTargetTransition", transition);
                    }
                    invocation.complete(projectBrokerTarget(method, result, current));
                }
            } catch (Exception malformed) { invocation.completeExceptionally(malformed); }
            deadline.operationFinished();
        });
        return invocation;
    }

    private String privateControllerKey() {
        UsbDevice observed = device;
        if (observed == null) return null;
        try {
            String serial = observed.getSerialNumber();
            if (serial == null || serial.trim().isEmpty() || observed != device) return null;
            // Kept only in this operation's closure; never serialized or logged.
            return observed.getVendorId() + ":" + observed.getProductId() + ":"
                + observed.getDeviceId() + ":" + observed.getDeviceName() + ":" + serial;
        } catch (RuntimeException unavailable) { return null; }
    }

    static void expireRelayInvocation(CompletableFuture<org.json.JSONObject> invocation) {
        QcUsbPlugin session = relaySession;
        if (session != null) session.demoProvider.expire(invocation);
        RelayDeadline deadline = RELAY_DEADLINES.get(invocation);
        if (deadline != null) deadline.expire();
    }

    static CompletableFuture<Void> relayInvocationRetirement(
        CompletableFuture<org.json.JSONObject> invocation
    ) {
        RelayDeadline deadline = RELAY_DEADLINES.get(invocation);
        // Demo futures have no USB deadline owner. Keep the relay's unpair/stop
        // cancellation ownership until their exact provider invocation ends.
        return deadline == null ? invocation.handle((result, error) -> (Void) null) : deadline.retirement();
    }

    private static void cancelDroppedRelayTasks(List<Runnable> dropped) {
        for (Runnable task : dropped) if (task instanceof QcRelayTrackedTask)
            ((QcRelayTrackedTask) task).cancelBeforeRun();
    }

    private static void cancelRelayOperationsForSession(QcUsbPlugin session) {
        for (RelayDeadline deadline : RELAY_DEADLINES.values())
            if (deadline.session() == session) deadline.cancelForShutdown();
    }

    private static void submitRelayTask(ExecutorService executor, RelayDeadline deadline, Runnable action) {
        // A scheduler callback can race the operation's final completion. Do
        // not let it enqueue new work after retirement has already released
        // the service slot.
        if (deadline != null && !deadline.taskQueued()) return;
        QcRelayTrackedTask task = new QcRelayTrackedTask(
            deadline == null ? null : deadline.retirementTracker(),
            action,
            deadline == null ? null : deadline::cancelForShutdown);
        try {
            executor.execute(task);
        } catch (RuntimeException error) {
            task.cancelBeforeRun();
            throw error;
        }
    }

    private static CompletableFuture<org.json.JSONObject> failedRelay(String code, String message) {
        return failedRelay(new RelayException(code, message));
    }

    private static CompletableFuture<org.json.JSONObject> failedRelay(RelayException failure) {
        CompletableFuture<org.json.JSONObject> value = new CompletableFuture<>();
        value.completeExceptionally(failure);
        return value;
    }

    private CompletableFuture<org.json.JSONObject> relayDisconnect() {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        submitRelayTask(commandIo, relayDeadline, () -> {
            try {
                if (result.isDone()) return;
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                if (relayDeadline != null && !beginRelayRecovery(relayDeadline)) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                synchronized (this) {
                    automaticReconnectGeneration.incrementAndGet();
                    closeConnection();
                }
                result.complete(connectionState("Quad Cortex session closed"));
            } catch (Exception error) { result.completeExceptionally(error); }
        });
        return result;
    }

    private CompletableFuture<org.json.JSONObject> relayReconnect(String detail) {
        return relayReconnect(detail, null);
    }

    private CompletableFuture<org.json.JSONObject> relayReconnect(String detail, String backupOrigin) {
        return relayReconnect(detail, backupOrigin, null);
    }

    private CompletableFuture<org.json.JSONObject> relayReconnect(
        String detail, String backupOrigin, Long recoveryGeneration
    ) {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        UsbDevice candidate = findQuadCortex();
        if (candidate == null) return failedRelay("DEVICE_NOT_FOUND", "No Quad Cortex was found over USB.");
        if (!manager.hasPermission(candidate)) return failedRelay("USB_PERMISSION_REQUIRED", "Quad Cortex USB permission is required.");
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        final long connectionIntent;
        synchronized (this) {
            if (recoveryGeneration != null
                && recoveryGeneration != automaticReconnectGeneration.get())
                return failedRelay("NOT_CONNECTED", "The automatic recovery was canceled by a newer connection request.");
            if (backupOrigin != null && reconnectInFlight != null)
                return failedRelay("NOT_CONNECTED", "The QC session is already being replaced; backup recovery was not started.");
            if (reconnectInFlight != null) return reconnectInFlight;
            connectionIntent = recoveryGeneration == null
                ? automaticReconnectGeneration.incrementAndGet() : recoveryGeneration;
            connecting = true;
            connectingSinceMs = monotonicMillis();
            reconnectInFlight = result;
        }
        // Keep the lifecycle barrier installed until the semantic seed has
        // completed (or the ready deadline fails). openDeviceAndHandshake()
        // returns after ResetCommsBuffers, before post-boot initialization and
        // the first preset have necessarily arrived. Clearing here used to let
        // a command race that interval and fail NOT_CONNECTED instead of being
        // queued behind this future.
        result.whenComplete((ignored, error) -> {
            synchronized (this) {
                if (reconnectInFlight == result) {
                    connecting = false;
                    reconnectInFlight = null;
                }
            }
        });
        QcPendingOperations.Entry<PendingReady> pending = pendingOperations.register(new PendingReady(detail, backupOrigin), result);
        pendingReady = pending;
        submitRelayTask(commandIo, relayDeadline, () -> {
            if (pending.result.isDone() || pendingReady != pending
                || relayAdmissionGuard.isTerminal()) return;
            if (relayDeadline != null && relayDeadline.isExpired()) {
                result.completeExceptionally(relayDeadlineExpired());
                return;
            }
            if (relayDeadline != null && !beginRelayRecovery(relayDeadline)) {
                result.completeExceptionally(relayDeadlineExpired());
                return;
            }
            try {
                openDeviceAndHandshake(candidate, pending, relayAdmissionGuard.epoch(), connectionIntent);
                resolvePendingReady();
            } catch (Exception error) {
                if (pendingReady == pending) pendingReady = null;
                pendingOperations.remove(pending);
                // A rejected origin has not touched USB. Cleanup must not close
                // the replacement connection that made the claim stale.
                if (backupOrigin == null || pending.operation.openingStarted) {
                    lastError = error.getMessage();
                    closeConnection();
                }
                result.completeExceptionally(new RelayException("USB_CONNECT_FAILED", error.getMessage()));
            }
        });
        pendingOperations.timeout(pending, QcUsbProfile.READY_WAIT_TIMEOUT_MS, keepalive,
            () -> new RelayException("READBACK_TIMEOUT", "The Quad Cortex did not finish synchronizing in time."));
        return result;
    }

    private void scheduleAutomaticReconnect(String detail) {
        scheduleAutomaticReconnect(detail, automaticReconnectGeneration.get());
    }

    private synchronized void scheduleAutomaticReconnect(String detail, long recoveryGeneration) {
        if (recoveryGeneration != automaticReconnectGeneration.get() || relayAdmissionGuard.isTerminal()) return;
        long delayMs = stateDecoder.sessionScheduleReconnect(monotonicMillis());
        keepalive.schedule(() -> {
            synchronized (this) {
                if (recoveryGeneration != automaticReconnectGeneration.get() || relayAdmissionGuard.isTerminal()) return;
                if (isReady()) return;
                UsbDevice candidate = findQuadCortex();
                long now = monotonicMillis();
                // Re-arm on every path that is not "already connected". Returning
                // without rescheduling ended this loop for good, and two of its
                // four exits did exactly that: a tick landing while a connect was
                // in flight, or before the backoff came due, killed recovery
                // permanently. Nothing else retries -- pressing Connect meets the
                // same `connecting` guard -- so the app could only be recovered by
                // force-stopping it, which is what a detach mid-handshake produced.
                if (connecting || !stateDecoder.sessionReconnectDue(now)
                    || candidate == null || !manager.hasPermission(candidate)) {
                    scheduleAutomaticReconnect(detail, recoveryGeneration);
                    return;
                }
                stateDecoder.sessionReconnectAttempted(now);
                relayReconnect(detail, null, recoveryGeneration).whenComplete((ignored, error) -> {
                    if (error != null) {
                        android.util.Log.w(
                            "QcUsbPlugin", "Automatic QC USB reconnect failed: " + error.getMessage());
                        scheduleAutomaticReconnect(detail, recoveryGeneration);
                    }
                });
            }
        }, delayMs, TimeUnit.MILLISECONDS);
    }

    private void resolvePendingReady() {
        QcPendingOperations.Entry<PendingReady> pending = pendingReady;
        if (pending == null || !isReady()
            || !stateDecoder.sessionSynchronized() || currentSetlist == null) return;
        if (pendingReady == pending) {
            pendingReady = null;
            if (pendingOperations.remove(pending)) {
                pending.result.complete(connectionState(pending.operation.detail));
            }
        }
    }

    private org.json.JSONObject connectionState(String detail) {
        JSObject state = new JSObject();
        state.put("phase", connection == null ? "disconnected"
            : stateDecoder.sessionSynchronized() && currentSetlist != null ? "ready" : "syncing");
        state.put("detail", detail);
        state.put("lastSync", connectedAt == 0 ? org.json.JSONObject.NULL : connectedAt);
        state.put("demo", false);
        return state;
    }

    private org.json.JSONObject relayStateEvents(org.json.JSONObject params) throws Exception {
        long after = params.has("afterSequence") ? params.getLong("afterSequence") : 0;
        int limit = params.has("limit") ? params.getInt("limit") : QcDomain.STATE_EVENT_DEFAULT_LIMIT;
        if (after < 0) throw new IllegalArgumentException("afterSequence must be a non-negative integer.");
        if (limit < 1 || limit > QcDomain.STATE_EVENT_MAXIMUM_LIMIT)
            throw new IllegalArgumentException("limit must be an integer from 1 through " + QcDomain.STATE_EVENT_MAXIMUM_LIMIT + ".");
        org.json.JSONArray frames;
        long latestSequence;
        synchronized (stateEventLock) {
            frames = stateEventLog.page(after, limit);
            latestSequence = nextStateSequence - 1;
        }
        JSObject result = new JSObject();
        result.put("native", true);
        result.put("latestSequence", latestSequence);
        result.put("streamId", stateStreamId);
        result.put("frames", frames);
        return result;
    }

    private org.json.JSONObject relayTempoClock() {
        JSObject clock = latestTempoClock;
        if (clock != null) return clock;
        JSObject unavailable = new JSObject();
        unavailable.put("available", false);
        return unavailable;
    }

    private CompletableFuture<org.json.JSONObject> relayCreateBackup(org.json.JSONObject params) throws Exception {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        // Validate with the shared native boundary before admitting an export
        // or sending its uncorrelated request. Invalid names must not consume
        // a complete device backup only to fail during final document assembly.
        String name = QcNativeStateDecoder.validateBackupName(params.optString("name", ""));
        if (pendingBackup != null && !pendingBackup.result.isDone()) return failedRelay("BACKUP_IN_PROGRESS", "A device backup is already in progress.");
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        long startedAt = monotonicMillis();
        String admission = stateDecoder.operationTicket(startedAt, QcUsbProfile.BACKUP_TOTAL_TIMEOUT_MS);
        admission = stateDecoder.backupStarted(admission, startedAt);
        QcPendingOperations.Entry<PendingBackup> pending = pendingOperations.register(new PendingBackup(name, admission), result);
        pendingBackup = pending;
        result.whenComplete((ignored, error) -> {
            pendingOperations.remove(pending);
        });
        issueBackupRequest(pending, admission, relayDeadline);
        scheduleBackupWatchdog(pending, MAINTENANCE_POLL_MS, relayDeadline);
        return result;
    }

    /** LocalBackup export is a single uncorrelated request. Never replay it. */
    private void issueBackupRequest(QcPendingOperations.Entry<PendingBackup> pending, String admission, RelayDeadline relayDeadline) {
        submitRelayTask(commandIo, relayDeadline, () -> {
            try {
                if (pendingBackup != pending || pending.result.isDone()
                    || pending.operation.recoveryStarted) return;
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    failPendingBackupRecovery(pending, relayDeadlineExpired());
                    return;
                }
                if (!beginRelayMutation(relayDeadline)) {
                    if (relayDeadline != null && relayDeadline.isExpired()) {
                        failPendingBackupRecovery(pending, relayDeadlineExpired());
                        return;
                    }
                    throw new RelayException("WRITE_QUARANTINED",
                        "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
                }
                if (!isReady()) throw new RelayException("NOT_CONNECTED", "Quad Cortex USB disconnected before the backup.");
                android.util.Log.i("QcUsbPlugin", "Sending single native backup request; includeReportId="
                    + includeReportId);
                writeMessage(stateDecoder.backupCommand(), includeReportId,
                    stateDecoder.prepareDispatch(admission, monotonicMillis()));
            } catch (Exception error) {
                failPendingBackupRecovery(pending, error);
            }
        });
    }

    private void scheduleBackupWatchdog(QcPendingOperations.Entry<PendingBackup> pending, long delayMs, RelayDeadline relayDeadline) {
        keepalive.schedule(() -> {
            if (pendingBackup != pending || pending.result.isDone()) return;
            PendingBackup operation = pending.operation;
            if (operation.recoveryStarted) return;
            try {
                JSObject decision = stateDecoder.backupAdvance(operation.admission, monotonicMillis());
                String action = decision.getString("action", "wait");
                if ("keepalive".equals(action)) {
                    submitRelayTask(commandIo, relayDeadline, () -> {
                        try {
                            if (pendingBackup != pending || pending.result.isDone() || operation.recoveryStarted) return;
                            // This is continuation traffic for the already-admitted backup operation,
                            // not a second mutation admission. Keep it alive until its native owner
                            // completes so a relay timeout does not strand the device in backup mode.
                            writeMessage(stateDecoder.keepaliveCommand(), includeReportId,
                                stateDecoder.prepareDispatch(operation.admission, monotonicMillis()));
                            stateDecoder.sessionKeepaliveSent(monotonicMillis());
                        } catch (Exception error) {
                            if (operation.recoveryStarted) return;
                            failPendingBackupRecovery(pending, error);
                        }
                    });
                } else if ("failed".equals(action)) {
                    pendingBackup = null;
                    stateDecoder.backupCancelled(pending.operation.admission);
                    if (pendingOperations.remove(pending)) pending.result.completeExceptionally(new RelayException(
                        "READBACK_TIMEOUT", decision.getString("error", "The native QC backup failed.")
                            + " Android observed " + operation.rawReports + " raw HID reports and "
                            + operation.decodedMessages + " decoded messages."));
                    return;
                }
                scheduleBackupWatchdog(pending, MAINTENANCE_POLL_MS, relayDeadline);
            } catch (Exception error) {
                failPendingBackupRecovery(pending, error);
            }
        }, Math.max(1, delayMs), TimeUnit.MILLISECONDS);
    }

    private org.json.JSONObject saveBackupDocument(org.json.JSONObject document, String requestedName) throws Exception {
        return QcBackupStorage.saveDocument(getContext(), document, requestedName);
    }

    /**
     * The gateway.status payload. Every consumer (qc-client's GatewayStatus,
     * the Windows broker, the Python gateway) reads capabilities as a JSON
     * array of strings. Android's org.json does not wrap a Java array passed to
     * put(), so handing it GeneratedGatewayMethods.CAPABILITIES directly
     * serialised the field as "[Ljava.lang.String;@..."; build a JSONArray.
     */
    static org.json.JSONObject systemStatus(boolean connected, boolean synchronizedSession,
        org.json.JSONObject usbDiagnostics) throws org.json.JSONException {
        return new org.json.JSONObject()
            .put("platform", "Rust Android USB gateway")
            .put("gatewayAvailable", true)
            .put("gatewayApiVersion", GeneratedGatewayMethods.API_VERSION)
            .put("capabilities", capabilitiesJson())
            .put("message", "Shared Rust QC engine active")
            .put("connected", connected)
            .put("synchronized", synchronizedSession)
            .put("transport", "android-usb-relay")
            .put("usbDiagnostics", usbDiagnostics);
    }

    static org.json.JSONArray capabilitiesJson() {
        return new org.json.JSONArray(java.util.Arrays.asList(GeneratedGatewayMethods.CAPABILITIES));
    }

    private CompletableFuture<org.json.JSONObject> relayInvoke(String method, org.json.JSONObject params, org.json.JSONObject expected) {
        try {
            if (flight != null) flight.activity(GeneratedGatewayMethods.contains(method)
                ? "gateway-dispatch:" + method : "gateway-dispatch:rejected");
            params = stateDecoder.mergeExpectedState(
                JSObject.fromJSONObject(params), JSObject.fromJSONObject(expected));
            GeneratedGatewayMethods.validateParams(method, params);
            String dispatch = GeneratedGatewayMethods.dispatchKind(method);
            if (relayAdmissionGuard.writesQuarantined() && isMutationDispatch(dispatch)) {
                return failedRelay(RelayException.quarantinedBeforeDispatch());
            }
            if ("SYSTEM".equals(dispatch)) {
                return CompletableFuture.completedFuture(systemStatus(isReady(),
                    stateDecoder.sessionSynchronized() && currentSetlist != null, usbDiagnostics()));
            }
            if ("RECONNECT".equals(dispatch)) return relayReconnect(
                "device.resetSession".equals(method) ? "Communication session reset" : "Quad Cortex handshake complete");
            if ("DISCONNECT".equals(dispatch)) return relayDisconnect();
            if (!isReady()) {
                CompletableFuture<org.json.JSONObject> reconnect = reconnectInFlight;
                if (reconnect != null) {
                    org.json.JSONObject deferredParams = params;
                    org.json.JSONObject deferredExpected = expected;
                    RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
                    CompletableFuture<org.json.JSONObject> deferred = new CompletableFuture<>();
                    reconnect.whenComplete((ignored, reconnectError) -> {
                        if (reconnectError != null) {
                            deferred.completeExceptionally(reconnectError);
                            return;
                        }
                        if (relayDeadline != null && relayDeadline.isExpired()) {
                            deferred.completeExceptionally(relayDeadlineExpired());
                            return;
                        }
                        try {
                            submitRelayTask(commandIo, relayDeadline, () -> {
                                CompletableFuture<org.json.JSONObject> resumed;
                                CURRENT_RELAY_DEADLINE.set(relayDeadline);
                                try { resumed = relayInvoke(method, deferredParams, deferredExpected); }
                                catch (RuntimeException error) {
                                    deferred.completeExceptionally(error);
                                    return;
                                } finally { CURRENT_RELAY_DEADLINE.remove(); }
                                resumed.whenComplete((value, error) -> {
                                    if (error == null) deferred.complete(value);
                                    else deferred.completeExceptionally(error);
                                });
                            });
                        } catch (RuntimeException error) {
                            deferred.completeExceptionally(error);
                        }
                    });
                    return deferred;
                }
                return failedRelay("NOT_CONNECTED", "Quad Cortex USB is not connected.");
            }
            if (!stateDecoder.sessionSynchronized()) {
                return failedRelay("STATE_UNAVAILABLE", "Quad Cortex USB is still synchronizing its authoritative state.");
            }
            // The device holds the session suspended (a backup or a reset is
            // running on the unit): refuse anything that would reach it, with
            // the reason, before planning. Retained reads and status continue.
            if (QcSuspensionGate.refuses(dispatch)) {
                String refusal = stateDecoder.suspensionRefusal();
                if (refusal != null) {
                    if (flight != null) flight.event("gateway-suspended:" + method);
                    return failedRelay(QcSuspensionGate.CODE, refusal);
                }
            }
            switch (dispatch) {
                case "SNAPSHOT": return CompletableFuture.completedFuture(relaySnapshot());
                case "CURRENT_PRESET": return relayCurrentPreset();
                case "STATE_EVENTS": return CompletableFuture.completedFuture(relayStateEvents(params));
                case "TEMPO_CLOCK": return CompletableFuture.completedFuture(relayTempoClock());
                case "CORRELATED_READ": return relayGatewayRead(method, params);
                // CorOS publishes these two families at handshake and then
                // answers writes with a sparse echo; that it never answers an
                // on-demand READ of type 9 or type 3 is BELIEVED (recorded without
                // a date or artifact). Serve them from the
                // same shared fold the Windows broker serves them from, so
                // neither host puts a request on the wire the device ignores.
                case "GENERAL_SETTINGS":
                    return CompletableFuture.completedFuture(
                        stateDecoder.publishedSettings(QcUsbProfile.MESSAGE_TYPE_GENERAL_SETTINGS));
                case "IO_SETTINGS":
                    return CompletableFuture.completedFuture(
                        stateDecoder.publishedSettings(QcUsbProfile.MESSAGE_TYPE_IO_SETTINGS));
                case "MODELS": return CompletableFuture.completedFuture(stateDecoder.modelList());
                case "RETAINED_CATALOG": return relayCatalogRead(method, params);
                case "MASTER_VOLUME":
                    if (currentMasterVolume < 0) return failedRelay("STATE_UNAVAILABLE", "The Quad Cortex has not reported master volume yet.");
                    return CompletableFuture.completedFuture(new org.json.JSONObject().put("value", currentMasterVolume).put("observedAt", currentMasterVolumeObservedAt));
                case "BLOCK_DETAILS": return relayBlockDetails(method, params);
                case "SET_DEVICE_NAME": return relaySetDeviceName(params);
                // mouse_frame is withheld on Android: a lone press leaves the pointer
                // down, and only the Windows broker tracks a held touch (it refuses
                // other gestures until a release or raw tap). drag_screen is no longer
                // withheld: it is now the same single DRAG frame as swipe_screen, which
                // begins no held touch and has nothing to release.
                case "TAP_SCREEN": return "device.mouseFrame".equals(method)
                    ? failedRelay("METHOD_NOT_ALLOWED", "mouse_frame is not supported by the Android host; it cannot track a touch that a press has left held, so it could leave the pointer down. Use the Windows host.")
                    : relayScreenGesture(method, params);
                case "BACKUP": return relayCreateBackup(params);
                case "PREVIEW_PARAMETER": return relayPreviewParameter(method, params);
                // undo and redo are one UndoRedo UPDATE frame and nothing after it:
                // no refresh read, no readback, no preflight, so they take the plain
                // single-frame lane like the host's other unread writes.
                case "PLANNED_WRITE": return "device.undo".equals(method) || "device.redo".equals(method)
                    ? relayPlannedGatewayWrite(method, params)
                    : relayPlannedGatewayWriteWithReadback(method, params);
                case "PRESET_WRITE": return relayPlannedGatewayWrite(method, params);
                case "PERSISTENT_WRITE": return relayPersistentGatewayWrite(method, params);
                default: return failedRelay("METHOD_NOT_ALLOWED", "The requested device operation is not supported by Android.");
            }
        } catch (RelayException error) {
            return failedRelay(error);
        } catch (Exception error) { return failedRelay("INVALID_ARGUMENT", error.getMessage() == null ? "Invalid device arguments." : error.getMessage()); }
    }

    private static boolean isMutationDispatch(String dispatch) {
        return QcGatewayTargetPolicy.isMutationDispatch(dispatch);
    }

    private CompletableFuture<org.json.JSONObject> relayPlannedGatewayWrite(
        String method, org.json.JSONObject params
    ) throws Exception {
        QcNativeStateDecoder.PlannedGatewayWrite plan = stateDecoder.gatewayPlan(method, JSObject.fromJSONObject(params));
        return executeRelayPlan(plan);
    }

    private CompletableFuture<org.json.JSONObject> relayPlannedGatewayWriteWithReadback(
        String method, org.json.JSONObject params
    ) throws Exception {
        QcNativeStateDecoder.PlannedGatewayWrite plan =
            stateDecoder.gatewayPlan(method, JSObject.fromJSONObject(params));
        CompletableFuture<QcNativeStateDecoder.GatewayWritePreflight> preflight =
            CompletableFuture.completedFuture(new QcNativeStateDecoder.GatewayWritePreflight(plan, false, null));
        if (plan.preflightMethod != null) {
            // Host policy (not a Cortex Control frame): no preflight sends a READ.
            // The guard compares the caller's expected values with state the
            // shared fold already holds (what the QC pushed) and fails closed,
            // sending nothing, when that state has not been published. The
            // Windows broker does exactly the same.
            org.json.JSONObject observation;
            try {
                observation = QcGatewayPreflight.authoritativeObservation(
                    plan.preflightObservationSource,
                    () -> stateDecoder.preflightObservation(method));
            } catch (Exception unavailable) {
                return failedRelay("STATE_UNAVAILABLE", unavailable.getMessage() == null
                    ? "The state this write's guard needs has not been published; nothing was sent."
                    : unavailable.getMessage());
            }
            preflight = CompletableFuture.completedFuture(
                stateDecoder.bindGatewayWritePreflight(plan, params, observation));
        }
        org.json.JSONObject guardedParams = new org.json.JSONObject(params.toString());
        return preflight.thenCompose(bound -> {
            try {
                if (bound.alreadyMatched) return CompletableFuture.completedFuture(new org.json.JSONObject()
                    .put("accepted", true).put("verified", true)
                    .put("verification", "authoritative_preflight")
                    .put("detail", "The requested setting already matched authoritative device state; no mutation was sent.")
                    .put("readback", bound.observation));
                QcNativeStateDecoder.PlannedGatewayWrite admittedPlan = bound.plan;
                QcSettingsEchoEvidence.Fence echoFence = new QcSettingsEchoEvidence.Fence();
                return executeRelayPlan(admittedPlan, -1, echoFence).thenCompose(writeResult -> admittedPlan.noOp || admittedPlan.readbackMethod == null
                    ? CompletableFuture.completedFuture(writeResult)
                    : relayGatewayWriteReadback(method, guardedParams, admittedPlan.readbackMethod,
                        stateDecoder.beginGatewayReadbackScope(admittedPlan.sessionFence, monotonicMillis(), admittedPlan.confirmationTimeoutMs), echoFence, 0, writeResult));
            } catch (Exception error) {
                return QcUsbPlugin.<org.json.JSONObject>failedFuture(error);
            }
        });
    }

    /**
     * Wait for the QC's own sparse echo of a type-9 / type-3 write by polling the retained
     * publication the shared fold merged it into. It sends no frame: the only readbacks that
     * remain are of these two published families. When the poll schedule or the budget runs
     * out the write was delivered and nothing confirmed it, so the caller gets the plain
     * accepted_unverified write result - not an error and not a quarantine.
     */
    private CompletableFuture<org.json.JSONObject> relayGatewayWriteReadback(
        String method, org.json.JSONObject params, String readMethod, String scope, QcSettingsEchoEvidence.Fence echoFence, int attempt,
        org.json.JSONObject writeResult
    ) {
        long delayMs = stateDecoder.gatewayReadbackRetryDelay(method, attempt);
        if (delayMs < 0) return CompletableFuture.completedFuture(writeResult);
        try {
            if (delayMs >= stateDecoder.gatewayReadScopeRemaining(scope, monotonicMillis())) {
                return CompletableFuture.completedFuture(writeResult);
            }
        } catch (Exception error) {
            // The scope's session changed or its budget ended: still not a failed write.
            return CompletableFuture.completedFuture(writeResult);
        }
        CompletableFuture<Void> delay = new CompletableFuture<>();
        if (delayMs == 0) delay.complete(null);
        else keepalive.schedule(() -> delay.complete(null), delayMs, TimeUnit.MILLISECONDS);
        return delay.thenCompose(ignored -> {
            try {
                int published = publishedSettingsType(readMethod);
                if (published < 0) return CompletableFuture.completedFuture(writeResult);
                QcSettingsEchoEvidence.Echo echo;
                synchronized (stateEventLock) { echo = settingsEchoEvidence.candidate(published, echoFence); }
                // A newer merged cache is insufficient: the actual echo must carry this field.
                if (echo == null || !stateDecoder.operationObservationIsFresh(echoFence.operation, echo.observationTicket)
                    || !stateDecoder.gatewayWriteEchoMatches(method, params, published, echo.payload)) {
                    return CompletableFuture.<org.json.JSONObject>completedFuture(null);
                }
                return CompletableFuture.completedFuture(stateDecoder.publishedSettings(published));
            }
            catch (Exception error) { return QcUsbPlugin.<org.json.JSONObject>failedFuture(error); }
        }).thenCompose(readback -> {
            if (readback == writeResult) return CompletableFuture.completedFuture(writeResult);
            if (readback == null || !stateDecoder.gatewayReadbackMatches(method, params, readback)) {
                return relayGatewayWriteReadback(
                    method, params, readMethod, scope, echoFence, attempt + 1, writeResult);
            }
            try {
                stateDecoder.gatewayReadScopeRemaining(scope, monotonicMillis());
                return CompletableFuture.completedFuture(new org.json.JSONObject()
                    .put("accepted", true).put("verified", false)
                    .put("verification", "accepted_unverified")
                    .put("observation", new org.json.JSONObject()
                        .put("kind", "matching_publication_after_native_admission").put("causallyCorrelated", false))
                    .put("detail", "A matching publication was observed after native admission, without request correlation; it cannot prove this write caused the value."));
            } catch (Exception error) {
                return CompletableFuture.completedFuture(writeResult);
            }
        });
    }

    private CompletableFuture<org.json.JSONObject> executeRelayPlan(
        QcNativeStateDecoder.PlannedGatewayWrite plan
    ) {
        return executeRelayPlan(plan, -1);
    }

    private CompletableFuture<org.json.JSONObject> executeRelayPlan(
        QcNativeStateDecoder.PlannedGatewayWrite plan,
        long expectedConnectionGeneration
    ) {
        return executeRelayPlan(plan, expectedConnectionGeneration, null);
    }

    private CompletableFuture<org.json.JSONObject> executeRelayPlan(
        QcNativeStateDecoder.PlannedGatewayWrite plan, long expectedConnectionGeneration,
        QcSettingsEchoEvidence.Fence echoFence
    ) {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        if (plan.midi) {
            final String admission;
            try {
                admission = stateDecoder.operationTicket(plan.sessionFence, monotonicMillis(),
                    QcUsbProfile.COMMAND_CONFIRMATION_TIMEOUT_MS);
            } catch (Exception error) { return failedFuture(error); }
            CompletableFuture<org.json.JSONObject> midi = relayMidi(plan.controller, plan.value, relayDeadline, admission);
            return midi.thenApply(result -> {
            try { return result.put("accepted", true).put("verified", false).put("detail", plan.detail)
                .put("verification", "accepted_unverified"); }
            catch (Exception error) { throw new java.util.concurrent.CompletionException(error); }
            });
        }
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        long hostStartedAtUnixMs = System.currentTimeMillis();
        QcPendingOperations.Entry<PendingGatewayTransaction> pending = null;
        PendingGatewayTransaction operation;
        final String admission;
        try {
            admission = stateDecoder.operationTicket(plan.sessionFence, monotonicMillis(),
                QcUsbProfile.COMMAND_CONFIRMATION_TIMEOUT_MS
                    + plan.interMessageIntervalMs * plan.messages.size());
            org.json.JSONObject verification = new org.json.JSONObject(plan.verificationJson);
            if (!plan.noOp && !plan.realtime && !"none".equals(verification.optString("kind"))) {
                operation = new PendingGatewayTransaction(
                    plan, 0, hostStartedAtUnixMs);
                pending = pendingOperations.register(operation, result);
            } else {
                operation = new PendingGatewayTransaction(plan, 0, hostStartedAtUnixMs);
            }
        } catch (Exception error) {
            result.completeExceptionally(error);
            return result;
        }
        operation.admission = admission;
        if (echoFence != null) echoFence.operation = admission;
        QcPendingOperations.Entry<PendingGatewayTransaction> registered = pending;
        submitRelayTask(commandIo, relayDeadline, () -> {
            try {
                if (result.isDone()) return;
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                if (relayAdmissionGuard.writesQuarantined()) throw new RelayException("WRITE_QUARANTINED",
                    "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
                if (!isReady()) throw new RelayException("NOT_CONNECTED", "Quad Cortex USB disconnected before the write.");
                if (expectedConnectionGeneration >= 0
                    && expectedConnectionGeneration != connectionGeneration.get()) {
                    throw new RelayException("STALE_STATE",
                        "The USB session changed after persistent-mutation planning; no mutation was sent.");
                }
                if (plan.noOp) {
                    stateDecoder.operationRemaining(admission, monotonicMillis());
                    result.complete(new org.json.JSONObject().put("accepted", true).put("verified", true)
                        .put("changed", false).put("requestSent", false)
                        .put("verification", "authoritative_preflight")
                        .put("detail", "The guarded footswitch assignment already matched; no mutation was sent."));
                    return;
                }
                org.json.JSONObject verification = new org.json.JSONObject(plan.verificationJson);
                // A hold is a touch-down, a wait and a touch-up: if the touch-up cannot
                // be written after the touch-down was, the writer sends one release itself.
                if ("device.longTapScreen".equals(plan.method)) {
                    if (relayDeadline != null && relayDeadline.isExpired()) {
                        result.completeExceptionally(relayDeadlineExpired());
                        return;
                    }
                    if (!beginRelayMutation(relayDeadline)) {
                        if (relayDeadline != null && relayDeadline.isExpired()) {
                            result.completeExceptionally(relayDeadlineExpired());
                            return;
                        }
                        throw new RelayException("WRITE_QUARANTINED",
                        "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
                    }
                    writeHoldMessages(plan.messages, plan.interMessageIntervalMs, admission);
                } else {
                    if (relayDeadline != null && relayDeadline.isExpired()) {
                        result.completeExceptionally(relayDeadlineExpired());
                        return;
                    }
                    if (!beginRelayMutation(relayDeadline)) {
                        if (relayDeadline != null && relayDeadline.isExpired()) {
                            result.completeExceptionally(relayDeadlineExpired());
                            return;
                        }
                        throw new RelayException("WRITE_QUARANTINED",
                        "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
                    }
                    writeMessages(plan.messages, plan.interMessageIntervalMs, admission);
                }
                operation.dispatchLatencyMs = Math.max(0, System.currentTimeMillis() - hostStartedAtUnixMs);
                if (!plan.noOp && !plan.realtime && !"none".equals(verification.optString("kind"))) {
                    operation.verificationId = stateDecoder.gatewayVerificationStarted(
                        plan, operation.afterSequence, monotonicMillis());
                }
                operation.dispatched = true;
                if (plan.realtime || "none".equals(verification.optString("kind"))) {
                    result.complete(new org.json.JSONObject().put("accepted", true).put("verified", false)
                        .put("detail", plan.detail).put("verification", "accepted_unverified")
                        .put("hostStartedAtUnixMs", hostStartedAtUnixMs)
                        .put("dispatchLatencyMs", operation.dispatchLatencyMs));
                } else {
                    long currentSequence;
                    synchronized (stateEventLock) { currentSequence = nextStateSequence - 1; }
                    advancePendingGatewayTransaction(registered, currentSequence, monotonicMillis());
                }
            } catch (Exception error) {
                if (registered != null) {
                    pendingOperations.remove(registered);
                    stateDecoder.gatewayVerificationCancelled(operation.verificationId);
                }
                result.completeExceptionally(error);
            }
        });
        if (registered == null) return result;
        result.whenComplete((ignored, error) ->
            stateDecoder.gatewayVerificationCancelled(operation.verificationId));
        return result;
    }

    private CompletableFuture<org.json.JSONObject> relayGatewayWorkflow(
        String method, org.json.JSONObject params
    ) throws Exception {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        long preparation = stateDecoder.beginPresetWorkflow(method, params, monotonicMillis());
        return new QcPresetWorkflowRunner<org.json.JSONObject>(
            () -> nextPresetWorkflowAction(preparation, relayDeadline),
            (task, delay) -> keepalive.schedule(task, delay, TimeUnit.MILLISECONDS),
            () -> stateDecoder.cancelPresetPreparation(preparation)).start();
    }

    private QcPresetWorkflowRunner.Step<org.json.JSONObject> nextPresetWorkflowAction(
        long preparation, RelayDeadline relayDeadline
    ) throws Exception {
        if (relayAdmissionGuard.writesQuarantined()) throw new RelayException("WRITE_QUARANTINED",
            "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
        if (relayDeadline != null && relayDeadline.isExpired()) {
            throw new RelayException("DEVICE_OUTCOME_UNKNOWN",
                "The persistent device workflow exceeded its relay deadline and was stopped.");
        }
        QcNativeStateDecoder.PresetWorkflowAction step =
            stateDecoder.advancePresetWorkflow(preparation, monotonicMillis());
        String kind = step.value.getString("kind");
        if ("finished".equals(kind)) {
            if (step.value.has("error")) {
                String error = step.value.getString("error");
                quarantineWrites(error);
                throw new RelayException("WRITE_OUTCOME_AMBIGUOUS", error);
            }
            return new QcPresetWorkflowRunner.Step<>(true, step.value.getJSONObject("result"), null, 0);
        }
        Runnable submit = null;
        if ("write".equals(kind)) {
            String operation = step.value.getJSONObject("operation").toString();
            long interval = step.value.getLong("interMessageIntervalMs");
            boolean preparationRead = step.value.optBoolean("preparation", false);
            submit = () -> {
                if (relayDeadline != null && relayDeadline.isExpired()) return;
                try {
                    submitRelayTask(commandIo, relayDeadline, () -> {
                        try {
                            if (preparationRead) {
                                if (relayDeadline != null && relayDeadline.isExpired()) return;
                                if (!beginRelayRead(relayDeadline)) return;
                            } else {
                                if (relayDeadline != null && relayDeadline.isExpired()) return;
                                if (!beginRelayMutation(relayDeadline)) throw new RelayException("WRITE_QUARANTINED",
                                    "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
                            }
                            writeMessages(step.messages, interval, operation);
                        }
                        catch (Exception error) {
                            // Native journal and fixed owner deadlines decide the result.
                            android.util.Log.w("QcUsbPlugin", "Preset workflow USB submission failed", error);
                        }
                    });
                } catch (java.util.concurrent.RejectedExecutionException error) {
                    if (preparationRead) throw error;
                    // Preserve the outstanding owner action until its original deadline.
                    // A queue failure must never cause a persistent replay.
                }
            };
        }
        return new QcPresetWorkflowRunner.Step<>(false, null, submit, step.value.optLong("delayMs", 10L));
    }

    private CompletableFuture<org.json.JSONObject> relayPersistentGatewayWrite(
        String method, org.json.JSONObject params
    ) {
        RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
        return persistentMutationGate.submit(() -> {
            if (relayDeadline != null && relayDeadline.isExpired()) {
                return failedRelay("DEVICE_OUTCOME_UNKNOWN", "The device operation exceeded its request deadline before dispatch.");
            }
            if (relayAdmissionGuard.writesQuarantined()) return failedRelay("WRITE_QUARANTINED",
                "A previous device write has an unknown outcome. Writes are disabled until the QC reconnects and synchronizes.");
            if (!isReady()) {
                return failedRelay("NOT_CONNECTED",
                    "Quad Cortex USB disconnected while the persistent mutation was queued.");
            }
            if (!stateDecoder.sessionSynchronized()) {
                return failedRelay("STATE_UNAVAILABLE",
                    "Quad Cortex USB state was no longer synchronized when the persistent mutation started.");
            }
            RelayDeadline previous = CURRENT_RELAY_DEADLINE.get();
            if (relayDeadline != null) CURRENT_RELAY_DEADLINE.set(relayDeadline);
            try { return relayGatewayWorkflow(method, params); }
            finally {
                if (previous == null) CURRENT_RELAY_DEADLINE.remove();
                else CURRENT_RELAY_DEADLINE.set(previous);
            }
        });
    }

    private CompletableFuture<org.json.JSONObject> relayGatewayRead(
        String method, org.json.JSONObject params
    ) throws Exception {
        boolean captureScreen = "device.captureScreen".equals(method);
        if (captureScreen && !captureScreenGate.tryAcquire()) {
            return failedRelay("READ_IN_PROGRESS", "A screen capture is already in progress.");
        }
        try {
            org.json.JSONObject readParams = new org.json.JSONObject(params.toString());
            RelayDeadline relayDeadline = CURRENT_RELAY_DEADLINE.get();
            CompletableFuture<org.json.JSONObject> read = relayGatewayReadWithRetry(method, readParams, relayDeadline);
            if (captureScreen) {
                read.whenComplete((ignored, error) -> captureScreenGate.release());
            }
            return read;
        } catch (Exception error) {
            if (captureScreen) captureScreenGate.release();
            throw error;
        }
    }

    private CompletableFuture<org.json.JSONObject> relayBlockDetails(
        String method, org.json.JSONObject params
    ) throws Exception {
        int row = params.getInt("row");
        boolean laneControl = "device.laneControlDetails".equals(method);
        String control = laneControl ? params.getString("control") : null;
        int column = laneControl ? -1 : params.getInt("column");
        stateDecoder.assertExpectedPreset(params);
        // Served from the pushed state mirror, exactly like get_current_preset:
        // no frame is sent. Host policy (not a Cortex Control frame): Cortex
        // Control sends a type-15 READ only at boot, so this no longer forces
        // one with a host request id. On CorOS 4.1.0 the unit pushes the Grid
        // echo within ~0.1 s of a parameter write and a full preset, scene,
        // position and Grid after an undo (measured 2026-10-07); whether other
        // firmware pushes as much is BELIEVED. Any expected_* guard in params
        // is compared with this same mirror.
        try {
            org.json.JSONObject details = control == null
                ? stateDecoder.blockDetails(row, column, params)
                : stateDecoder.laneControlDetails(row, control, params);
            return CompletableFuture.completedFuture(details);
        } catch (Exception error) {
            return QcUsbPlugin.<org.json.JSONObject>failedFuture(error);
        }
    }

    private CompletableFuture<org.json.JSONObject> relayGatewayReadWithRetry(
        String method, org.json.JSONObject readParams, RelayDeadline relayDeadline
    ) throws Exception {
        String scope = stateDecoder.beginGatewayReadScope(method, readParams, monotonicMillis());
        return relayGatewayReadOnCurrentSession(method, readParams, scope, relayDeadline).handle((value, error) -> {
            if (error == null) return CompletableFuture.completedFuture(value);
            if (relayDeadline != null && relayDeadline.isExpired()) {
                return QcUsbPlugin.<org.json.JSONObject>failedFuture(relayDeadlineExpired());
            }
            Throwable cause = unwrapCompletion(error);
            if ("device.diagnostics".equals(method) || "device.looperStatus".equals(method)
                || !(cause instanceof RelayException)
                || !"READBACK_TIMEOUT".equals(((RelayException) cause).code)) {
                return QcUsbPlugin.<org.json.JSONObject>failedFuture(cause);
            }
            try {
                // A shorter component wait may expire before the logical read.
                // A retry can use remaining time, never a new session or budget.
                stateDecoder.gatewayReadScopeRemaining(scope, monotonicMillis());
                return relayGatewayReadOnCurrentSession(method, readParams, scope, relayDeadline);
            } catch (Exception retryError) {
                return QcUsbPlugin.<org.json.JSONObject>failedFuture(cause);
            }
        }).thenCompose(result -> result);
    }

    private static Throwable unwrapCompletion(Throwable error) {
        Throwable current = error;
        while (current instanceof CompletionException && current.getCause() != null) {
            current = current.getCause();
        }
        return current;
    }

    private static <T> CompletableFuture<T> failedFuture(Throwable error) {
        CompletableFuture<T> result = new CompletableFuture<>();
        result.completeExceptionally(error);
        return result;
    }

    private CompletableFuture<org.json.JSONObject> relayGatewayReadOnCurrentSession(
        String method, org.json.JSONObject params, String scope, RelayDeadline relayDeadline
    ) throws Exception {
        long startedAtMs = monotonicMillis();
        QcNativeStateDecoder.PlannedGatewayRead plan = stateDecoder.gatewayRead(
            method, JSObject.fromJSONObject(params), scope, startedAtMs);
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        String admission = stateDecoder.operationTicket(plan.sessionFence, startedAtMs, plan.timeoutMs);
        QcPendingOperations.Entry<PendingGatewayRead> pending = pendingOperations.register(
            new PendingGatewayRead(plan, scope, startedAtMs), result);
        result.whenComplete((ignored, error) -> {
            pendingOperations.remove(pending);
        });
        submitRelayTask(commandIo, relayDeadline, () -> {
            try {
                // Timers and disconnect callbacks can finish the caller while
                // this command is still queued. Never transmit that old read.
                if (result.isDone()) return;
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                if (!beginRelayRead(relayDeadline)) {
                    if (relayDeadline != null && relayDeadline.isExpired()) result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                if (!stateDecoder.readEpochCurrent(plan)) throw new RelayException("NOT_CONNECTED", "The QC session changed before read submission.");
                if (stateDecoder.requestExpired(pending.operation.startedAtMs, plan.timeoutMs, monotonicMillis())) {
                    throw new RelayException("READBACK_TIMEOUT", "The QC read expired before USB submission.");
                }
                if (!isReady()) throw new RelayException("NOT_CONNECTED", "Quad Cortex USB disconnected before the read.");
                pending.operation.afterMessageSequence = usbDiagnostics.snapshot().messagesReceived;
                writeMessages(plan.messages, 0, admission);
            } catch (Exception error) {
                pendingOperations.remove(pending);
                result.completeExceptionally(error);
            }
        });
        pendingOperations.timeout(pending, Math.max(1L, plan.timeoutMs - (monotonicMillis() - startedAtMs)), keepalive,
            () -> new RelayException("READBACK_TIMEOUT", "The QC did not provide the requested reply in time."));
        if (plan.followupMethod == null) return result;
        return result.thenCompose(primary -> {
            try {
                return relayGatewayReadOnCurrentSession(
                    plan.followupMethod, new org.json.JSONObject(), scope, relayDeadline).thenApply(followup -> {
                        try {
                            org.json.JSONObject combined = stateDecoder.composeGlobalTempoSettings(primary, followup);
                            stateDecoder.gatewayReadScopeRemaining(scope, monotonicMillis());
                            return combined;
                        }
                        catch (Exception error) { throw new CompletionException(error); }
                    });
            } catch (Exception error) {
                return QcUsbPlugin.<org.json.JSONObject>failedFuture(error);
            }
        });
    }

    // One Version UPDATE frame and nothing after it, like the Windows broker:
    // no identity readback ladder, so the result is accepted_unverified once the
    // frame is delivered. Hardware verification of the rename is pending.
    private CompletableFuture<org.json.JSONObject> relaySetDeviceName(org.json.JSONObject params) throws Exception {
        return relayPlannedGatewayWrite("device.setDeviceName", params);
    }

    private CompletableFuture<org.json.JSONObject> relayScreenGesture(
        String method, org.json.JSONObject params
    ) throws Exception {
        return relayPlannedGatewayWriteWithReadback(method, params);
    }

    private CompletableFuture<org.json.JSONObject> relayPreviewParameter(String method, org.json.JSONObject params) throws Exception {
        double value = params.optDouble("value", Double.NaN);
        return relayPlannedGatewayWrite(method, params)
            .thenApply(result -> {
                try {
                    return result.put("acceptedValue", value);
                } catch (org.json.JSONException error) {
                    throw new IllegalStateException("Could not encode the parameter preview result.", error);
                }
            });
    }

    private CompletableFuture<org.json.JSONObject> relayCatalogRead(
        String method, org.json.JSONObject params
    ) throws Exception {
        boolean refresh = !"device.captures".equals(method) && !"device.irs".equals(method)
            && params.optBoolean("refresh", false);
        if (refresh) {
            // Refuse, as the Windows broker does, rather than buying a new USB
            // session behind the caller's back. Host policy (not a
            // desktop-application frame): the retained directory (the boot
            // publication plus the File updates the unit pushes) is served and
            // no explicit refresh is implemented. The unit DOES answer a bare
            // directory read on demand (ESTABLISHED, CorOS 4.1.0, 2026-10-07),
            // so this is a missing feature, not a device limit. Reconnecting here
            // cost a full ~12 s handshake and silently discarded every other
            // session-scoped observation.
            return failedRelay("PRESET_CATALOG_REFRESH_UNAVAILABLE",
                GeneratedHostPolicy.PRESET_CATALOG_REFRESH_UNAVAILABLE);
        }
        long startedAtMs = monotonicMillis();
        String readContext = stateDecoder.catalogContext(null);
        String requestedSetlist = firstText(params, "setlistKey", "setlist_key");
        if ("device.irs".equals(method) && params.has("folder") && !params.isNull("folder")
            && !(params.get("folder") instanceof String)) {
            throw new IllegalArgumentException("folder must be a string or null");
        }
        String catalogKey = "device.captures".equals(method) ? ""
            : "device.irs".equals(method) ? (params.isNull("folder") ? "" : params.getString("folder"))
            : "device.listPresetFolders".equals(method) ? ""
            : "device.listPresetSlots".equals(method) ? stateDecoder.snapshot().optString("setlistKey", "")
            : requestedSetlist == null ? currentSetlist : requestedSetlist;
        if (catalogKey == null) throw new IllegalStateException("No active preset setlist has been synchronized.");
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        QcPendingOperations.Entry<PendingCatalogRead> pending = pendingOperations.register(
            new PendingCatalogRead(method, catalogKey, readContext, startedAtMs,
                QcUsbProfile.PRESET_SYNC_TIMEOUT_MS), result);
        // The startup publication and later native actions feed the same retained
        // catalog. A cache miss does not authorize another USB directory request.
        resolvePendingCatalogRead(pending);
        return result;
    }

    private CompletableFuture<org.json.JSONObject> relayMidi(int controller, int value,
            RelayDeadline relayDeadline, String admission) {
        if (value < 0 || value > 127) return failedRelay("INVALID_ARGUMENT", "MIDI value is outside the supported range.");
        final long admittedGeneration;
        final UsbDeviceConnection admittedConnection;
        final UsbEndpoint admittedEndpoint;
        synchronized (this) {
            admittedGeneration = connectionGeneration.get();
            admittedConnection = midiConnection;
            admittedEndpoint = midiOutputEndpoint;
            if (admittedConnection == null || admittedEndpoint == null)
                return failedRelay("MIDI_NOT_AVAILABLE", "Quad Cortex USB-MIDI is unavailable.");
        }
        CompletableFuture<org.json.JSONObject> result = new CompletableFuture<>();
        long queuedAtUnixMs = System.currentTimeMillis();
        long queuedAtMonotonicMs = monotonicMillis();
        final long queuedGeneration = connectionGeneration.get();
        submitRelayTask(midiIo, relayDeadline, () -> {
            try {
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                long remaining = QcUsbProfile.PERFORMANCE_MIDI_GAP_MS
                    - (monotonicMillis() - lastMidiCommandAt);
                if (remaining > 0) Thread.sleep(remaining);
                if (relayDeadline != null && relayDeadline.isExpired()) {
                    result.completeExceptionally(relayDeadlineExpired());
                    return;
                }
                byte[] packet = {
                    (byte) QcUsbProfile.MIDI_USB_EVENT_PACKET_HEADER,
                    (byte) QcUsbProfile.MIDI_CONTROL_CHANGE_STATUS,
                    (byte) controller,
                    (byte) value
                };
                final int written;
                // Closing and publishing a USB session use this monitor too.
                // A queued performance command must retain its native plan's
                // provenance instead of borrowing a replacement connection.
                synchronized (this) {
                    written = QcMidiWritePolicy.writeIfCurrent(admittedGeneration, connectionGeneration.get(),
                        admittedConnection, midiConnection, admittedEndpoint, midiOutputEndpoint,
                        () -> QcMidiWriteGate.send(queuedGeneration, connectionGeneration.get(),
                        isReady() && stateDecoder.sessionSynchronized(), stateDecoder.suspensionRefusal(),
                        () -> stateDecoder.prepareDispatch(admission, monotonicMillis()),
                        () -> {
                            if (!beginRelayMutation(relayDeadline)) {
                                if (relayDeadline != null && relayDeadline.isExpired()) throw relayDeadlineExpired();
                                throw RelayException.quarantinedBeforeDispatch();
                            }
                        },
                        () -> {
                            UsbDeviceConnection activeMidiConnection = midiConnection;
                            if (activeMidiConnection == null || midiOutputEndpoint == null)
                                throw new RelayException("MIDI_NOT_AVAILABLE", "Quad Cortex USB-MIDI disconnected before the write.");
                            return admittedConnection.bulkTransfer(admittedEndpoint, packet, packet.length, MIDI_WRITE_TIMEOUT_MS);
                        }));
                }
                lastMidiCommandAt = monotonicMillis();
                if (written != packet.length) throw new RelayException("MIDI_WRITE_FAILED", "The complete MIDI packet was not written.");
                lastMidiQueueDelayMs = Math.max(0, monotonicMillis() - queuedAtMonotonicMs);
                maxMidiQueueDelayMs = Math.max(maxMidiQueueDelayMs, lastMidiQueueDelayMs);
                result.complete(new org.json.JSONObject()
                    .put("accepted", true).put("verified", false)
                    .put("verification", "accepted_unverified")
                    .put("hostStartedAtUnixMs", queuedAtUnixMs)
                    .put("dispatchLatencyMs", lastMidiQueueDelayMs)
                    .put("detail", "Performance MIDI command accepted; live USB state will reconcile the result."));
            } catch (Exception error) { result.completeExceptionally(error); }
        });
        return result;
    }

    private org.json.JSONObject relaySnapshot() throws Exception {
        return decorateSnapshot(stateDecoder.snapshot());
    }

    private org.json.JSONObject decorateSnapshot(org.json.JSONObject snapshot) throws Exception {
        snapshot.put("connected", isReady()).put("synchronized", stateDecoder.sessionSynchronized())
            .put("position", snapshot.optInt("presetPosition", currentPosition))
            .put("observedAt", lastStateAt);
        if (currentMasterVolume < 0) snapshot.put("masterVolume", org.json.JSONObject.NULL);
        return snapshot;
    }

    private CompletableFuture<org.json.JSONObject> relayCurrentPreset() throws Exception {
        org.json.JSONObject observation = stateDecoder.currentPresetObservation();
        org.json.JSONObject current = observation.getJSONObject("snapshot");
        if (current.optBoolean("gridLayoutComplete", false)) {
            return CompletableFuture.completedFuture(decorateSnapshot(
                new org.json.JSONObject(current.toString())));
        }

        // Sparse structural echoes can leave the cached block list incomplete.
        // Only this explicit read repairs that cache: send one bare type-15 READ
        // and return only a newer complete publication from the same session.
        // The usual complete-cache path above remains no-frame. Event-triggered
        // UI reconciliation can call this path; periodic polls and automatic
        // post-write verification do not.
        org.json.JSONObject priorVersion = current.optJSONObject("stateVersion");
        if (priorVersion == null || priorVersion.optString("session", "").isEmpty()) {
            return failedRelay("STATE_UNAVAILABLE",
                "The incomplete Grid has no session version for a safe current-preset refresh.");
        }
        final String priorSession = priorVersion.getString("session");
        final long priorRevision = priorVersion.getLong("revision");
        final long priorFullPresetRevision = observation.getLong("fullPresetRevision");
        final long priorConnectionGeneration = connectionGeneration.get();
        synchronized (currentPresetRefreshLock) {
            if (currentPresetGridRefresh != null && !currentPresetGridRefresh.isDone()) {
                return currentPresetGridRefresh.thenApply(value -> {
                    try { return new org.json.JSONObject(value.toString()); }
                    catch (org.json.JSONException error) { throw new CompletionException(error); }
                });
            }
            String scope = stateDecoder.beginGatewayReadScope(
                "device.currentPreset", new org.json.JSONObject(), monotonicMillis());
            RelayDeadline deadline = CURRENT_RELAY_DEADLINE.get();
            CompletableFuture<org.json.JSONObject> refresh = relayGatewayReadOnCurrentSession(
                "device.currentPreset", new org.json.JSONObject(), scope, deadline)
                .thenApply(ack -> {
                    try {
                        stateDecoder.gatewayReadScopeRemaining(scope, monotonicMillis());
                        org.json.JSONObject latestObservation = stateDecoder.currentPresetObservation();
                        org.json.JSONObject latest = latestObservation.getJSONObject("snapshot");
                        org.json.JSONObject version = latest.optJSONObject("stateVersion");
                        if (!latest.optBoolean("gridLayoutComplete", false)
                            || connectionGeneration.get() != priorConnectionGeneration
                            || latestObservation.getLong("fullPresetRevision") <= priorFullPresetRevision
                            || version == null
                            || !priorSession.equals(version.optString("session", ""))
                            || version.optLong("revision", 0) <= priorRevision) {
                            throw new RelayException("STATE_UNAVAILABLE",
                                "The Quad Cortex did not publish a new complete Grid in the current session; the cached layout remains incomplete.");
                        }
                        return decorateSnapshot(new org.json.JSONObject(latest.toString()));
                    } catch (Exception error) {
                        throw new CompletionException(error);
                    }
                });
            currentPresetGridRefresh = refresh;
            refresh.whenComplete((value, error) -> {
                synchronized (currentPresetRefreshLock) {
                    if (currentPresetGridRefresh == refresh) currentPresetGridRefresh = null;
                }
            });
            return refresh;
        }
    }

    private static String firstText(org.json.JSONObject value, String... keys) {
        for (String key : keys) {
            if (value.isNull(key)) continue;
            String text = value.optString(key, "").trim();
            if (!text.isEmpty()) return text;
        }
        return null;
    }

    private static int firstInt(org.json.JSONObject primary, org.json.JSONObject fallback, String... keys) {
        for (String key : keys) { if (primary.has(key)) return primary.optInt(key, -1); if (fallback.has(key)) return fallback.optInt(key, -1); }
        return -1;
    }

    @PluginMethod
    public void scan(PluginCall call) {
        JSArray found = new JSArray();
        for (UsbDevice candidate : manager.getDeviceList().values()) {
            if (!QcUsbDevicePolicy.isQuadCortex(candidate)) continue;
            JSObject entry = new JSObject();
            entry.put("deviceId", candidate.getDeviceId());
            entry.put("name", candidate.getProductName() == null ? getContext().getString(R.string.device_name) : candidate.getProductName());
            entry.put("manufacturer", candidate.getManufacturerName());
            entry.put("permission", manager.hasPermission(candidate));
            entry.put("connecting", connecting);
            entry.put("reconnectInFlight", reconnectInFlight != null);
            entry.put("interfaces", candidate.getInterfaceCount());
            found.put(entry);
        }
        JSObject result = new JSObject();
        result.put("devices", found);
        result.put("connected", isReady());
        result.put("synchronized", isReady() && stateDecoder.sessionSynchronized() && currentSetlist != null);
        if (flight != null) flight.event(found.length() == 0 ? "scan-absent" : "scan-available");
        call.resolve(result);
    }

    @PluginMethod
    public synchronized void internetForwarding(PluginCall call) {
        call.resolve(new JSObject().put("enabled", internetForwardingEnabled));
    }

    @PluginMethod
    public void setInternetForwarding(PluginCall call) {
        synchronized (internetForwardingSettingLock) {
        Boolean enabled = call.getBoolean("enabled");
        if (enabled == null) { call.reject("An enabled value is required."); return; }
        if (!getContext().getSharedPreferences("qc-usb", Context.MODE_PRIVATE).edit()
                .putBoolean(INTERNET_FORWARDING_SETTING, enabled).commit()) {
            call.reject("Could not save Internet forwarding setting.");
            return;
        }
        if (!enabled) internetForwarder.setEnabled(false);
        synchronized (this) {
            stateDecoder.setCloudForwardEnabled(enabled);
            if (enabled) internetForwarder.setEnabled(true);
            internetForwardingEnabled = enabled;
        }
        call.resolve(new JSObject().put("enabled", enabled));
        }
    }

    @PluginMethod
    public void connect(PluginCall call) {
        if (flight != null) flight.event("connect-requested");
        UsbDevice candidate = findQuadCortex();
        if (candidate == null) {
            call.reject("No Quad Cortex was found over USB.", "DEVICE_NOT_FOUND");
            return;
        }
        if (isReady() && device != null
            && device.getDeviceId() == candidate.getDeviceId()) {
            resolveConnected(call, candidate);
            return;
        }
        if (connecting && !clearStaleConnectingLatch()) {
            call.reject("The Quad Cortex USB connection is already starting.", "CONNECT_IN_PROGRESS");
            return;
        }
        if (manager.hasPermission(candidate)) {
            openAndHandshake(candidate, call);
            return;
        }
        // Only a deliberate press may raise a dialog. A device attaching while
        // the app runs already puts Android's own confirmation on screen, via
        // the manifest's USB_DEVICE_ATTACHED filter, and asking as well is what
        // produced two prompts for one plug. That grant comes back through
        // systemGrantedAttachment instead.
        if (!call.getData().optBoolean("allowPermissionPrompt", true)) {
            call.reject("Quad Cortex USB permission is required.", "USB_PERMISSION_REQUIRED");
            return;
        }
        // A prompt is already up for this device; a second request would orphan
        // the first call, whose promise would then never settle.
        if (pendingConnect != null) {
            call.reject("The Quad Cortex USB permission prompt is already open.", "CONNECT_IN_PROGRESS");
            return;
        }
        pendingConnect = call;
        if (flight != null) flight.event("usb-permission-requested");
        PendingIntent permissionIntent = PendingIntent.getBroadcast(
            getContext(), 0, new Intent(USB_PERMISSION).setPackage(getContext().getPackageName()),
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE
        );
        manager.requestPermission(candidate, permissionIntent);
    }

    /** Release a `connecting` flag that no live attempt is holding.
     *
     * The flag is raised under a lock and lowered by the completion of the work
     * it guards, so work that never completes leaves it raised and every later
     * connect is refused with CONNECT_IN_PROGRESS -- including the one the
     * person asks for by pressing Connect. The ready wait bounds how long an
     * attempt may legitimately run, so a flag older than that is not a race to
     * be respected. Returns true when it cleared one. */
    private boolean clearStaleConnectingLatch() {
        synchronized (this) {
            if (!connecting) return true;
            if (reconnectInFlight != null) return false;
            long heldMs = monotonicMillis() - connectingSinceMs;
            if (heldMs <= QcUsbProfile.READY_WAIT_TIMEOUT_MS) return false;
            if (flight != null) flight.event("connecting-latch-cleared");
            connecting = false;
            return true;
        }
    }

    /** Refuse an outstanding prompt. An answer must never outlive the session
     * it was asked for, and a caller left waiting forever wedges the screen. */
    private void rejectPendingConnect(String message) {
        PluginCall waiting = pendingConnect;
        pendingConnect = null;
        if (waiting != null) waiting.reject(message, "USB_PERMISSION_REQUIRED");
    }

    @PluginMethod
    public void disconnect(PluginCall call) {
        if (flight != null) flight.event("disconnect-requested");
        synchronized (this) {
            automaticReconnectGeneration.incrementAndGet();
            closeConnection();
        }
        call.resolve();
    }

    @PluginMethod
    public void diagnostics(PluginCall call) {
        JSObject result = usbDiagnostics();
        if (flight != null) result.put("flightRecorder", flight.snapshot());
        call.resolve(result);
    }

    /** Record that a mutation reached the wire but its device outcome could
     * not be confirmed. The Android analog of the desktop broker's
     * `Gateway::quarantine_writes` (services/device-broker/src/worker.rs) and
     * the classifier it acts on, `error_has_ambiguous_write_outcome`
     * (services/device-broker/src/rpc.rs): a write that was dispatched but
     * whose readback timed out, mismatched, or otherwise never confirmed.
     * Reads are unaffected; callers surface this from `usbDiagnostics()`
     * exactly as the desktop broker's `usbDiagnostics.writesQuarantined` /
     * `writeQuarantineReason` do. */
    private void quarantineWrites(String reason) {
        relayAdmissionGuard.quarantineWrites(reason);
        if (flight != null) flight.event("write-quarantined");
    }

    private boolean beginRelayMutation(RelayDeadline deadline) {
        if (deadline != null) deadline.assertTarget();
        return relayAdmissionGuard.beginMutation(deadline == null ? null : deadline.state());
    }

    private boolean beginRelayRead(RelayDeadline deadline) {
        if (deadline != null) deadline.assertTarget();
        return relayAdmissionGuard.beginRead(deadline == null ? null : deadline.state());
    }

    private boolean beginRelayRecovery(RelayDeadline deadline) {
        if (deadline != null) deadline.assertTarget();
        boolean admitted = relayAdmissionGuard.beginRecovery(deadline == null ? null : deadline.state());
        if (admitted && deadline != null) deadline.recoveryAdmitted();
        return admitted;
    }

    private static RelayException relayDeadlineExpired() {
        return new RelayException("DEVICE_OUTCOME_UNKNOWN",
            "The device operation exceeded its request deadline. Its outcome is unknown; do not retry it automatically.");
    }

    int expireRelayDeadline(RelayDeadline deadline) {
        int previous = relayAdmissionGuard.expire(deadline.state());
        if (previous == QcRelayDeadlineState.MUTATION_ADMITTED) {
            if (flight != null) flight.event("write-quarantined");
        }
        return previous;
    }

    private JSObject usbDiagnostics() {
        JSObject result = new JSObject();
        boolean synchronizedSession = isReady() && stateDecoder.sessionSynchronized() && currentSetlist != null;
        String phase = connection == null ? (connecting ? "connecting" : "disconnected")
            : synchronizedSession ? "ready" : "syncing";
        result.put("connected", connection != null);
        result.put("phase", phase);
        result.put("synchronized", synchronizedSession);
        result.put("writesQuarantined", relayAdmissionGuard.writesQuarantined());
        if (relayAdmissionGuard.writeQuarantineReason() != null)
            result.put("writeQuarantineReason", relayAdmissionGuard.writeQuarantineReason());
        // The shared owner's session-scoped record, as the desktop broker
        // reports it: a disconnect or a new session has already cleared it.
        try {
            JSObject suspension = stateDecoder.suspension();
            result.put("suspension", suspension);
            result.put("deviceSuspended", !suspension.isNull("active"));
        } catch (Exception error) {
            result.put("deviceSuspended", false);
        }
        result.put("device", device == null || device.getProductName() == null ? getContext().getString(R.string.device_name) : device.getProductName());
        QcUsbDiagnostics.Snapshot counters = usbDiagnostics.snapshot();
        result.put("sessionEpoch", counters.sessionEpoch);
        result.put("messagesReceived", counters.messagesReceived);
        result.put("messagesReceivedByType", messageCountsJson(counters.messagesReceivedByType));
        result.put("rawReportsReceived", counters.rawReportsReceived);
        result.put("messagesSent", counters.messagesSent);
        result.put("messagesSentByType", messageCountsJson(counters.messagesSentByType));
        result.put("decodeErrors", decodeErrors);
        result.put("expectedWriteStalls", expectedWriteStalls);
        result.put("terminalHidWriteFailures", terminalHidWriteFailures);
        result.put("lastMessageType", lastMessageType);
        result.put("connectedAt", connectedAt);
        result.put("setlistKnown", currentSetlist != null);
        result.put("presetPosition", currentPosition);
        result.put("modelCount", stateDecoder.modelCount());
        result.put("readAttempts", readAttempts);
        result.put("negativeReads", negativeReads);
        result.put("interfaceId", selectedInterfaceId);
        result.put("inputEndpointAddress", selectedInputEndpointAddress);
        result.put("inputMaxPacketSize", selectedInputMaxPacketSize);
        result.put("reportBytes", includeReportId
            ? QcNativeStateDecoder.REPORT_SIZE
            : QcNativeStateDecoder.REPORT_SIZE - 1);
        result.put("midiAvailable", midiOutputEndpoint != null);
        result.put("separateMidiConnection", midiConnection != null && midiConnection != connection);
        result.put("midiInterfaceId", midiInterface == null ? -1 : midiInterface.getId());
        result.put("midiOutputEndpointAddress", midiOutputEndpoint == null ? -1 : midiOutputEndpoint.getAddress());
        result.put("lastMidiQueueDelayMs", lastMidiQueueDelayMs);
        result.put("maxMidiQueueDelayMs", maxMidiQueueDelayMs);
        result.put("lastStateAt", lastStateAt);
        result.put("lastHidWriteDurationMs", lastHidWriteDurationMs);
        result.put("maxHidWriteDurationMs", maxHidWriteDurationMs);
        result.put("lastHidWriteResult", lastHidWriteResult);
        result.put("lastHidWriteIncludedReportId", lastHidWriteIncludedReportId);
        if (lastGatewayReadMismatch != null) result.put("lastGatewayReadMismatch", lastGatewayReadMismatch);
        UsbRequest[] inputRequests = activeInputRequests;
        result.put("readerRequestActive", inputRequests != null && inputRequests.length > 0);
        result.put("readerRequestCount", inputRequests == null ? 0 : inputRequests.length);
        result.put("readerWaiting", readerWaiting);
        result.put("readerExitedAt", readerExitedAt);
        if (lastReaderError != null) result.put("lastReaderError", lastReaderError);
        if (lastError != null) result.put("lastError", lastError);
        return result;
    }

    private static JSObject messageCountsJson(java.util.Map<Integer, Long> counts) {
        JSObject result = new JSObject();
        counts.forEach((messageType, count) -> result.put(Integer.toString(messageType), count));
        return result;
    }

    @PluginMethod
    public void gatewayInvoke(PluginCall call) {
        String method = call.getString("method", "");
        if (!GeneratedGatewayMethods.contains(method)) {
            call.reject("The requested gateway method is not part of the generated contract.", "METHOD_NOT_ALLOWED");
            return;
        }
        JSObject params = call.getObject("params");
        JSObject expected = call.getObject("expectedState");
        try {
            QcGatewayTargetPolicy.requireAccepted(method,
                Boolean.TRUE.equals(call.getBoolean("requireAcceptedBrokerTarget", false)), call.getData().has("expectedBrokerTarget"));
        } catch (RelayException missingContext) { call.reject(missingContext.getMessage(), missingContext.code); return; }
        QcBrokerTarget captured = currentBrokerTarget();
        if (captured != null && !"physical".equals(captured.source)) captured = null;
        if (call.getData().has("expectedBrokerTarget")) {
            final QcBrokerTarget requested;
            try { requested = QcBrokerTarget.parse(call.getData().opt("expectedBrokerTarget")); }
            catch (IllegalArgumentException malformed) { call.reject(malformed.getMessage(), "INVALID_ARGUMENT"); return; }
            if (!requested.same(captured)) {
                call.reject("The accepted physical QC target is no longer active.", "TARGET_SUPERSEDED");
                return;
            }
        }
        invokePhysicalGateway(method, params == null ? new JSObject() : params,
            expected == null ? new JSObject() : expected, captured).whenComplete((result, error) -> {
                error = unwrapCompletion(error);
                if (error == null) {
                    try {
                        targetMetadata(method, result);
                        GeneratedGatewayMethods.validateResult(method, result);
                        call.resolve(JSObject.fromJSONObject(result));
                    }
                    catch (Exception conversionError) {
                        call.reject("The gateway result did not match the generated contract.", "MALFORMED_RESPONSE", conversionError);
                    }
                }
                else if (error instanceof RelayException) {
                    RelayException relayError = (RelayException) error;
                    JSObject data = new JSObject().put("retryable", relayError.retryable());
                    Boolean userActionRequired = RelayException.userActionRequired(relayError.code);
                    if (userActionRequired != null) data.put("userActionRequired", userActionRequired.booleanValue());
                    if (relayError.requestSent != null || relayError.writesQuarantined != null) {
                        JSObject details = new JSObject();
                        if (relayError.requestSent != null) details.put("requestSent", relayError.requestSent.booleanValue());
                        if (relayError.writesQuarantined != null) details.put("writesQuarantined", relayError.writesQuarantined.booleanValue());
                        data.put("details", details);
                    }
                    call.reject(error.getMessage(), relayError.code, relayError, data);
                }
                else if (error instanceof Exception)
                    call.reject(error.getMessage() == null ? "The gateway operation failed." : error.getMessage(),
                        QcSuspensionGate.code(error, "DEVICE_ERROR"), (Exception) error);
                else call.reject("The gateway operation failed.", "DEVICE_ERROR");
            });
    }

    private void openAndHandshake(UsbDevice candidate, PluginCall call) {
        final long connectionIntent;
        synchronized (this) {
            if (isReady() && device != null
                && device.getDeviceId() == candidate.getDeviceId()) {
                resolveConnected(call, candidate);
                return;
            }
            if (connecting) {
                call.reject("The Quad Cortex USB connection is already starting.", "CONNECT_IN_PROGRESS");
                return;
            }
            connecting = true;
            connectingSinceMs = monotonicMillis();
            connectionIntent = automaticReconnectGeneration.incrementAndGet();
        }
        submitRelayTask(commandIo, CURRENT_RELAY_DEADLINE.get(), () -> {
            try {
                openDeviceAndHandshake(candidate, null, relayAdmissionGuard.epoch(), connectionIntent);
                resolveConnected(call, candidate);
            } catch (Exception error) {
                lastError = error.getMessage();
                closeConnection();
                call.reject(error.getMessage(), "USB_CONNECT_FAILED", error);
            } finally {
                connecting = false;
            }
        });
    }

    private void openDeviceAndHandshake(
        UsbDevice candidate, QcPendingOperations.Entry<?> preservedOperation, long expectedLifecycleEpoch,
        long connectionIntent
    ) throws Exception {
        // Activity recreation can leave the previous plugin's UsbRequest reader
        // unwinding after its connection is closed. Wait off the UI thread so
        // the replacement never claims the interface while that reader still
        // owns the old handle.
        if (relayAdmissionGuard.isTerminal() || expectedLifecycleEpoch != relayAdmissionGuard.epoch()
            || connectionIntent != automaticReconnectGeneration.get()) {
            throw new IllegalStateException("QC Remote is stopping; the USB connection attempt was canceled.");
        }
        if (!QcUsbReaderTeardown.process().await(QcUsbProfile.INITIAL_SYNC_TIMEOUT_MS)) {
            if (flight != null) flight.event("previous-reader-shutdown-timeout");
            throw new IllegalStateException(
                "A previous Quad Cortex USB reader is still stopping. Retry Connect in a moment.");
        }
        // All opens run on commandIo. Hold the same monitor as external closes
        // while claiming the completed attempt and beginning its teardown.
        synchronized (this) {
            if (relayAdmissionGuard.isTerminal() || expectedLifecycleEpoch != relayAdmissionGuard.epoch()
                || connectionIntent != automaticReconnectGeneration.get()) {
                throw new IllegalStateException("QC Remote is stopping; the USB connection attempt was canceled.");
            }
            PendingReady ready = preservedOperation != null && preservedOperation.operation instanceof PendingReady
                ? (PendingReady) preservedOperation.operation : null;
            if (ready != null && ready.backupOrigin != null) {
                if (preservedOperation.result.isDone() || pendingReady != preservedOperation)
                    throw new IllegalStateException("Backup recovery is no longer pending.");
                stateDecoder.claimBackupRecovery(ready.backupOrigin);
            }
            if (ready != null) ready.openingStarted = true;
            if (flight != null) flight.event("transport-open-started");
            closeConnection(preservedOperation);
        }
        UsbInterface selected = null;
        UsbEndpoint selectedInput = null;
        for (int index = 0; index < candidate.getInterfaceCount(); index++) {
            UsbInterface iface = candidate.getInterface(index);
            if (iface.getInterfaceClass() != UsbConstants.USB_CLASS_HID) continue;
            UsbEndpoint candidateInput = null;
            for (int endpointIndex = 0; endpointIndex < iface.getEndpointCount(); endpointIndex++) {
                UsbEndpoint endpoint = iface.getEndpoint(endpointIndex);
                if (endpoint.getDirection() == UsbConstants.USB_DIR_IN && endpoint.getType() == UsbConstants.USB_ENDPOINT_XFER_INT) candidateInput = endpoint;
            }
            if (iface.getId() == 5) {
                selected = iface;
                selectedInput = candidateInput;
                break;
            }
            if (selected == null) {
                selected = iface;
                selectedInput = candidateInput;
            }
        }
        if (selected == null) throw new IllegalStateException("The Quad Cortex HID interface was not found.");
        if (selectedInput == null) throw new IllegalStateException("The Quad Cortex HID input endpoint was not found.");
        UsbDeviceConnection opened = manager.openDevice(candidate);
        UsbDeviceConnection openedMidi = null;
        boolean published = false;
        try {
        if (opened == null || !opened.claimInterface(selected, true)) {
            if (opened != null) opened.close();
            throw new IllegalStateException("Could not claim the Quad Cortex HID interface.");
        }
        if (flight != null) flight.event("hid-interface-claimed");
        UsbInterface selectedMidi = null;
        UsbEndpoint selectedMidiOutput = null;
        for (int index = 0; index < candidate.getInterfaceCount(); index++) {
            UsbInterface iface = candidate.getInterface(index);
            if (iface.getInterfaceClass() != UsbConstants.USB_CLASS_AUDIO || iface.getInterfaceSubclass() != 3) continue;
            for (int endpointIndex = 0; endpointIndex < iface.getEndpointCount(); endpointIndex++) {
                UsbEndpoint endpoint = iface.getEndpoint(endpointIndex);
                if (endpoint.getDirection() == UsbConstants.USB_DIR_OUT && endpoint.getType() == UsbConstants.USB_ENDPOINT_XFER_BULK) {
                    selectedMidi = iface;
                    selectedMidiOutput = endpoint;
                    break;
                }
            }
            if (selectedMidiOutput != null) break;
        }
        if (selectedMidi != null) {
            openedMidi = manager.openDevice(candidate);
            if (openedMidi != null && openedMidi.claimInterface(selectedMidi, true)) {
                if (flight != null) flight.event("midi-interface-claimed");
            } else if (openedMidi != null) {
                openedMidi.close();
                openedMidi = null;
            }
        }
        final UsbDeviceConnection publishHidConnection = opened;
        final UsbDeviceConnection publishMidiConnection = openedMidi;
        final UsbInterface publishHidInterface = selected;
        final UsbEndpoint publishInputEndpoint = selectedInput;
        final UsbInterface publishMidiInterface = selectedMidi;
        final UsbEndpoint publishMidiOutputEndpoint = selectedMidiOutput;
        synchronized (this) {
        boolean accepted = publishConnectionAttempt(expectedLifecycleEpoch, connectionIntent, () -> {
        device = candidate;
        connection = publishHidConnection;
        midiConnection = publishMidiConnection;
        midiInterface = publishMidiConnection == null ? null : publishMidiInterface;
        midiOutputEndpoint = publishMidiConnection == null ? null : publishMidiOutputEndpoint;
        transportClosing = false;
        hidInterface = publishHidInterface;
        inputEndpoint = publishInputEndpoint;
        selectedInterfaceId = publishHidInterface.getId();
        selectedInputEndpointAddress = publishInputEndpoint.getAddress();
        selectedInputMaxPacketSize = publishInputEndpoint.getMaxPacketSize();
        usbDiagnostics.beginSession();
        decodeErrors = 0;
        expectedWriteStalls = 0;
        terminalHidWriteFailures = 0;
        lastMessageType = -1;
        lastError = null;
        relayAdmissionGuard.resetForNewSession();
        readAttempts = 0;
        negativeReads = 0;
        connectedAt = System.currentTimeMillis();
        lastMidiCommandAt = 0;
        lastMidiQueueDelayMs = 0;
        maxMidiQueueDelayMs = 0;
        lastHidWriteDurationMs = 0;
        maxHidWriteDurationMs = 0;
        lastStateAt = 0;
        readerWaiting = false;
        readerExitedAt = 0;
        lastReaderError = null;
        lifecyclePublishedConnected = false;
        lifecyclePublishedSynchronized = false;
        nativeConnectionToken = stateDecoder.sessionOpened(monotonicMillis());
        startReader();
        });
        if (!accepted) throw new IllegalStateException("QC Remote stopped while the USB connection was opening.");
        published = true;
        }
        if (!published) throw new IllegalStateException("USB connection was not published.");
        if (flight != null) flight.event("reader-started");
        if (relayAdmissionGuard.isTerminal() || expectedLifecycleEpoch != relayAdmissionGuard.epoch()) return;
        performHandshake();
        } finally {
            if (!published) {
                try { if (openedMidi != null) openedMidi.close(); } catch (Exception ignored) {}
                try { if (opened != null) opened.close(); } catch (Exception ignored) {}
            }
        }
    }

    // Publication is the final admission boundary for every open, explicit or automatic.
    // A manual Disconnect can retire an attempt while its OS claim is still returning.
    private synchronized boolean publishConnectionAttempt(
        long expectedLifecycleEpoch, long connectionIntent, Runnable publication
    ) {
        if (connectionIntent != automaticReconnectGeneration.get()) return false;
        return relayAdmissionGuard.publish(expectedLifecycleEpoch, publication);
    }

    private void resolveConnected(PluginCall call, UsbDevice candidate) {
        JSObject result = new JSObject();
        result.put("connected", isReady());
        result.put("synchronized", stateDecoder.sessionSynchronized() && currentSetlist != null);
        result.put("name", candidate.getProductName() == null ? getContext().getString(R.string.device_name) : candidate.getProductName());
        result.put("deviceId", candidate.getDeviceId());
        call.resolve(result);
    }

    private boolean isReady() {
        return connection != null && stateDecoder.sessionConnected();
    }

    private void performHandshake() {
        if (flight != null) flight.event("handshake-started");
        long handshakeGeneration = connectionGeneration.get();
        int attempts = 0;
        boolean answered = false;
        while (!answered) {
            if (transportClosing || connection == null
                || connectionGeneration.get() != handshakeGeneration) {
                throw new IllegalStateException("The Quad Cortex USB connection closed during its handshake.");
            }
            String session = UUID.randomUUID().toString().replace("-", "");
            QcNativeStateDecoder.HandshakeDecision handshake =
                stateDecoder.handshakeAttempt(monotonicMillis(), session);
            if (handshake.kind == QcNativeStateDecoder.HandshakeDecision.TIMED_OUT) break;
            if (handshake.kind == QcNativeStateDecoder.HandshakeDecision.WAIT) {
                try {
                    Thread.sleep(10L);
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                    throw new IllegalStateException("The Quad Cortex USB handshake was interrupted.", error);
                }
                continue;
            }
            attempts = handshake.attempt;
            if (flight != null) flight.event("handshake-attempt-" + attempts);
            // The shared transport selects the same 129-byte-first HID layout
            // probe on Windows and Android, retaining the 128-byte fallback for
            // Android stacks which omit the report-ID byte from SET_REPORT.
            includeReportId = handshake.includeReportId;
            resetReply = new CountDownLatch(1);
            try {
                // performHandshake already runs on commandIo. Drain inline so
                // the reset is not queued behind the handshake's own wait.
                drainLifecycle();
            } catch (Exception error) {
                throw new IllegalStateException("Could not encode the QC USB reset command.", error);
            }
            try {
                answered = resetReply.await(QcUsbProfile.HANDSHAKE_ATTEMPT_TIMEOUT_MS, TimeUnit.MILLISECONDS);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException("The Quad Cortex USB handshake was interrupted.", error);
            } finally {
                resetReply = null;
            }
        }
        if (!answered) {
            if (flight != null) flight.event("handshake-failed");
            throw new IllegalStateException("The Quad Cortex did not answer after " + attempts + " USB handshake attempts.");
        }
        if (flight != null) flight.event("handshake-reply");
        if (flight != null) flight.event("session-validated");
        scheduleSilentSessionWatchdog();
    }

    /** Retire a session that validated and then delivered nothing.
     *
     * Claiming the interface can succeed against an endpoint the kernel has
     * not finished reaping, and the handshake then completes against nothing:
     * interface claimed, handshake answered, session validated, and not one
     * frame afterwards. Reinstalling while the QC is attached reproduces it
     * every time, because the replaced process is killed with the connection
     * open and the new one opens before the old handle is gone. Waiting ten
     * seconds before opening the app avoids it, which is not something a
     * person can be asked to know.
     *
     * Nothing noticed: the session reported connected, the screen waited for
     * state that could not arrive, and only force-stopping the app recovered
     * it. Hand it to the ordinary terminal-failure path instead, which closes
     * the transport and re-arms the reconnect loop. */
    private synchronized void scheduleSilentSessionWatchdog() {
        final long baseline = usbDiagnostics.snapshot().messagesReceived;
        final QcSilentSessionWatchdog watchdog = new QcSilentSessionWatchdog(
            connectionGeneration.get(), baseline, connectionGeneration::get,
            () -> usbDiagnostics.snapshot().messagesReceived, this::isReady,
            () -> terminalTransportFailed(
                "The Quad Cortex accepted the USB session but sent no state.", "session-silent"));
        keepalive.schedule(() -> {
            // Closing and publishing a connection hold this same monitor.
            // A timer created by an older handshake cannot retire its successor.
            synchronized (this) { watchdog.run(); }
        }, SILENT_SESSION_TIMEOUT_MS, TimeUnit.MILLISECONDS);
    }

    private void requestLifecycleDrain() {
        lifecycleDrainRequested.set(true);
        if (!lifecycleDrainScheduled.compareAndSet(false, true)) return;
        commandIo.execute(() -> {
            try {
                while (lifecycleDrainRequested.getAndSet(false)) drainLifecycle();
            } finally {
                lifecycleDrainScheduled.set(false);
                // Close the race with an observation arriving as this task exits.
                if (lifecycleDrainRequested.get()) requestLifecycleDrain();
            }
        });
    }

    /** Runs exclusively on commandIo, which also serializes public HID writes. */
    private void drainLifecycle() {
        long generation = connectionGeneration.get();
        if (connection == null) return;
        try {
            QcNativeStateDecoder.LifecycleBatch batch;
            while ((batch = stateDecoder.lifecycleNextBatch(
                monotonicMillis(), System.currentTimeMillis())) != null) {
                try {
                    for (QcNativeStateDecoder.EncodedMessage message : batch.messages) {
                        if (connection == null || !stateDecoder.lifecycleBatchCurrent(batch)) break;
                        writeMessage(message, includeReportId, batch.dispatchTicket);
                    }
                    stateDecoder.lifecycleDispatched(batch, true, monotonicMillis());
                } catch (Exception error) {
                    // Terminal I/O may already have cleared the JNI lifecycle.
                    // A stale receipt cannot fail a replacement connection.
                    stateDecoder.lifecycleDispatched(batch, false, monotonicMillis());
                    throw error;
                }
            }
            QcNativeStateDecoder.LifecycleStatus status = stateDecoder.lifecyclePoll(monotonicMillis());
            if (status.error != null) {
                lastError = "QC lifecycle " + status.phase + ": " + status.error;
                if (flight != null) flight.event("startup-" + status.phase);
                synchronized (this) {
                    if (connection != null && generation == connectionGeneration.get())
                        scheduleAutomaticReconnect(lastError);
                }
                return;
            }
            boolean connected = stateDecoder.sessionConnected();
            boolean synchronizedState = stateDecoder.sessionSynchronized();
            if (connected != lifecyclePublishedConnected
                || synchronizedState != lifecyclePublishedSynchronized) {
                lifecyclePublishedConnected = connected;
                lifecyclePublishedSynchronized = synchronizedState;
                if (connected) {
                    commandNotBeforeMs = monotonicMillis() + QcUsbProfile.POST_INITIALIZATION_WRITE_DELAY_MS;
                    resolvePendingReady();
                }
            }
        } catch (Exception error) {
            lastError = "Could not submit QC lifecycle command: " + error.getMessage();
            synchronized (this) {
                if (connection != null && generation == connectionGeneration.get())
                    terminalTransportFailed(lastError, "lifecycle-write-failed");
            }
        }
    }

    private void writeMessages(java.util.List<QcNativeStateDecoder.EncodedMessage> messages) {
        // Only graceful teardown uses this unfenced two-message drain.
        for (QcNativeStateDecoder.EncodedMessage message : messages) {
            writeMessage(message, includeReportId, null);
        }
    }

    private void writeMessages(
        java.util.List<QcNativeStateDecoder.EncodedMessage> messages,
        long interMessageIntervalMs,
        String admission
    ) {
        for (int index = 0; index < messages.size(); index++) {
            writeMessage(messages.get(index), includeReportId,
                stateDecoder.prepareDispatch(admission, monotonicMillis()));
            if (interMessageIntervalMs > 0 && index + 1 < messages.size()) {
                try {
                    Thread.sleep(interMessageIntervalMs);
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                    throw new IllegalStateException("The QC screen gesture was interrupted.", error);
                }
            }
        }
        // A started frame finishes under the transport's framing policy, but
        // late completion cannot renew the caller's budget or report success.
        stateDecoder.operationRemaining(admission, monotonicMillis());
    }

    /**
     * Write a hold's two frames, the touch-down (PRESS) and, after the wait, the
     * touch-up (RELEASE). If a frame after the touch-down fails, the pointer may
     * still be down on the unit, so this sends ONE more RELEASE (the same frame
     * as the touch-up, which did nothing when nothing was pending on a menu
     * button or row, observed on CorOS 4.1.0, 2026-10-06), but only on the very
     * USB connection the hold began on: a different connection is a different
     * session and nothing is sent into it. A failure of the touch-down itself gets
     * no release, since nothing is known to be pressed. The error says whether the
     * release went out; a failed frame is otherwise never replayed.
     */
    private void writeHoldMessages(
        java.util.List<QcNativeStateDecoder.EncodedMessage> messages,
        long interMessageIntervalMs,
        String admission
    ) {
        long generation = connectionGeneration.get();
        int accepted = 0;
        try {
            for (int index = 0; index < messages.size(); index++) {
                writeMessage(messages.get(index), includeReportId,
                    stateDecoder.prepareDispatch(admission, monotonicMillis()));
                accepted = index + 1;
                if (interMessageIntervalMs > 0 && index + 1 < messages.size()) {
                    try {
                        Thread.sleep(interMessageIntervalMs);
                    } catch (InterruptedException error) {
                        Thread.currentThread().interrupt();
                        throw new IllegalStateException("The QC screen gesture was interrupted.", error);
                    }
                }
            }
            stateDecoder.operationRemaining(admission, monotonicMillis());
        } catch (RuntimeException error) {
            // Nothing is known to be pressed unless the touch-down was accepted, and
            // once every frame was accepted there is nothing left to release.
            if (accepted < 1 || accepted >= messages.size()) throw error;
            String reason = error.getMessage() == null ? error.getClass().getSimpleName() : error.getMessage();
            try {
                if (connectionGeneration.get() != generation || !isReady()) {
                    throw new IllegalStateException("the USB connection the hold began on is gone");
                }
                writeMessage(messages.get(messages.size() - 1));
            } catch (RuntimeException releaseError) {
                throw new IllegalStateException(reason + "; the touch-up could not be sent: "
                    + (releaseError.getMessage() == null ? releaseError.getClass().getSimpleName() : releaseError.getMessage()), error);
            }
            throw new IllegalStateException(reason + "; the touch was released", error);
        }
    }

    private synchronized void writeMessage(QcNativeStateDecoder.EncodedMessage message) {
        writeMessage(message, includeReportId, stateDecoder.prepareDispatch(null, monotonicMillis()));
    }

    private synchronized void writeMessage(
        QcNativeStateDecoder.EncodedMessage message, boolean withReportId, String dispatchTicket
    ) {
        writeMessage(message, withReportId, dispatchTicket, 0);
    }

    private synchronized void writeForwardingMessage(long task, QcNativeStateDecoder.EncodedMessage message) {
        if (!internetForwarder.current(task) || !reading || transportClosing || connection == null)
            throw new IllegalStateException("Internet forwarding session ended.");
        writeMessage(message, includeReportId, null, task);
    }

    private synchronized void writeMessage(
        QcNativeStateDecoder.EncodedMessage message, boolean withReportId, String dispatchTicket, long forwardingTask
    ) {
        long diagnosticEpoch = usbDiagnostics.snapshot().sessionEpoch;
        long stabilizationDelayMs = commandNotBeforeMs - monotonicMillis();
        if (stabilizationDelayMs > 0) {
            try {
                Thread.sleep(stabilizationDelayMs);
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                throw new IllegalStateException("The first QC command was interrupted during stabilization.", error);
            }
        }
        commandNotBeforeMs = 0;
        List<byte[]> framedReports = forwardingTask == 0
            ? stateDecoder.encodeReports(message, withReportId, dispatchTicket, monotonicMillis())
            : stateDecoder.encodeForwardingReports(forwardingTask, message, withReportId, monotonicMillis());
        String delivery = dispatchTicket == null ? null : stateDecoder.prepareDelivery(
            dispatchTicket, message.messageType, framedReports.size(), monotonicMillis());
        try {
            if (delivery != null) stateDecoder.deliveryAdmitted(delivery, monotonicMillis());
            if (flight != null && forwardingTask == 0) flight.outbound(message.messageType, framedReports.size());
            for (int reportIndex = 0; reportIndex < framedReports.size(); reportIndex++) {
                byte[] report = framedReports.get(reportIndex);
                int reportTimeout = HID_WRITE_TIMEOUT_MS;
                if (forwardingTask != 0) {
                    reportTimeout = QcHidWritePolicy.forwardingWriteBudget(reportIndex > 0,
                        internetForwarder.current(forwardingTask),
                        () -> stateDecoder.forwardingWriteBudgetMs(forwardingTask),
                        detail -> terminalTransportFailed(detail, "forwarding-dispatch-revoked"));
                }
                if (connection == null || hidInterface == null) throw new IllegalStateException("Quad Cortex USB disconnected during write.");
                if (dispatchTicket != null) {
                    boolean began = reportIndex > 0;
                    int nativeReportIndex = reportIndex;
                    QcHidWritePolicy.validateDispatch(began,
                        () -> stateDecoder.beginDeliveryReport(delivery, nativeReportIndex, monotonicMillis()),
                        detail -> terminalTransportFailed(detail, "native-dispatch-revoked"));
                }
                UsbDeviceConnection activeConnection = connection;
                UsbDevice activeDevice = device;
                long activeGeneration = connectionGeneration.get();
                long writeStartedAt = monotonicMillis();
                int written;
                try {
                    written = activeConnection.controlTransfer(
                        0x21, 0x09, (2 << 8) | QcNativeStateDecoder.OUT_REPORT_ID,
                        hidInterface.getId(), report, report.length, reportTimeout);
                } catch (RuntimeException error) {
                    String detail = "USB HID write failed: "
                        + (error.getMessage() == null
                            ? error.getClass().getSimpleName() : error.getMessage());
                    if (delivery != null) stateDecoder.finishDeliveryReport(
                        delivery, reportIndex, QcHidWritePolicy.Outcome.TERMINAL, monotonicMillis());
                    terminalHidWriteFailed(detail);
                    throw new IllegalStateException(detail, error);
                }
                lastHidWriteDurationMs = monotonicMillis() - writeStartedAt;
                maxHidWriteDurationMs = Math.max(maxHidWriteDurationMs, lastHidWriteDurationMs);
                lastHidWriteResult = written;
                lastHidWriteIncludedReportId = withReportId;
                // A healthy -1 is the QC's accepted data stage followed by its
                // deliberate status-stage STALL. Preserve every remaining report.
                // Android cannot distinguish that STALL from a generic errno, so
                // only independent terminal state or a short write is fatal.
                QcHidWritePolicy.Outcome outcome = QcHidWritePolicy.classifyAndPropagate(
                    written,
                    report.length,
                    isDeviceStillAttached(activeDevice),
                    connection == activeConnection && connectionGeneration.get() == activeGeneration,
                    readerTerminatedGeneration == activeGeneration,
                    this::terminalHidWriteFailed);
                if (delivery != null) stateDecoder.finishDeliveryReport(delivery, reportIndex, outcome, monotonicMillis());
                if (outcome == QcHidWritePolicy.Outcome.TERMINAL) {
                    throw new IllegalStateException(lastError);
                }
                if (outcome == QcHidWritePolicy.Outcome.EXPECTED_STATUS_STALL) {
                    expectedWriteStalls++;
                }
                if ((reportIndex + 1) % QcUsbProfile.HID_REPORT_BATCH_SIZE == 0) {
                    try {
                        Thread.sleep(QcUsbProfile.HID_REPORT_BATCH_DELAY_MS);
                    } catch (InterruptedException error) {
                        Thread.currentThread().interrupt();
                        IllegalStateException interrupted = new IllegalStateException(
                            "The QC HID report batch was interrupted.", error);
                        if (forwardingTask != 0) QcHidWritePolicy.rejectInterruptedForwardingFrame(
                            reportIndex + 1 == framedReports.size(), interrupted,
                            detail -> terminalTransportFailed(detail, "forwarding-write-interrupted"));
                        throw interrupted;
                    }
                }
            }
            if (delivery != null) stateDecoder.requireDeliveryComplete(delivery, monotonicMillis());
        } finally {
            if (delivery != null) stateDecoder.stopPendingDelivery(delivery, monotonicMillis());
            if (forwardingTask != 0) for (byte[] report : framedReports) Arrays.fill(report, (byte) 0);
        }
        if (forwardingTask == 0) usbDiagnostics.messageSent(diagnosticEpoch, message.messageType);
    }

    private void startReader() {
        if (inputEndpoint == null) return;
        reading = true;
        long nativeConnection = nativeConnectionToken;
        long diagnosticEpoch = usbDiagnostics.snapshot().sessionEpoch;
        long generation = connectionGeneration.incrementAndGet();
        readerTerminatedGeneration = -1;
        readerIo.execute(() -> {
            UsbDeviceConnection activeConnection = connection;
            UsbEndpoint activeEndpoint = inputEndpoint;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                readInputReportsAsync(activeConnection, activeEndpoint, generation, nativeConnection, diagnosticEpoch);
                return;
            }
            try {
                while (readerIsActive(activeConnection, generation)) {
                    byte[] buffer = new byte[QcNativeStateDecoder.REPORT_SIZE];
                    readAttempts++;
                    int count = activeConnection.bulkTransfer(activeEndpoint, buffer, buffer.length, 250);
                    long receivedAtMs = monotonicMillis();
                    if (count <= 0) {
                        if (count < 0) negativeReads++;
                        continue;
                    }
                    long reportEpoch = stateDecoder.captureReportEpoch(nativeConnection);
                    consumeInputReport(buffer, count, nativeConnection, reportEpoch, diagnosticEpoch, receivedAtMs, generation);
                }
            } catch (Exception error) {
                if (readerIsActive(activeConnection, generation)) {
                    lastReaderError = error.getClass().getName() + ": " + error.getMessage();
                    lastError = "QC HID reader stopped: " + lastReaderError;
                    android.util.Log.e("QcUsbPlugin", lastError, error);
                }
            } finally {
                boolean recoverReader = readerIsActive(activeConnection, generation);
                if (recoverReader) readerTerminatedGeneration = generation;
                readerExitedAt = System.currentTimeMillis();
                recoverUnexpectedReaderExit(activeConnection, generation);
            }
        });
    }

    private boolean readerIsActive(UsbDeviceConnection activeConnection, long generation) {
        return reading && generation == connectionGeneration.get() && connection == activeConnection;
    }

    private boolean isDeviceStillAttached(UsbDevice expectedDevice) {
        if (manager == null || expectedDevice == null) return false;
        try {
            for (UsbDevice candidate : manager.getDeviceList().values()) {
                if (candidate.getDeviceId() == expectedDevice.getDeviceId()) return true;
            }
        } catch (RuntimeException ignored) {
            // Losing access to enumeration while a write is in flight is not
            // evidence that the ambiguous -1 was the QC's accepted STALL.
        }
        return false;
    }

    private void terminalHidWriteFailed(String detail) {
        terminalHidWriteFailures++;
        terminalTransportFailed(detail, "hid-write-terminal");
    }

    private synchronized void terminalTransportFailed(String detail, String event) {
        lastError = detail;
        if (flight != null) flight.event(event);
        closeConnection(null, false);
        scheduleAutomaticReconnect(detail);
    }

    @RequiresApi(Build.VERSION_CODES.O)
    private void readInputReportsAsync(
        UsbDeviceConnection activeConnection, UsbEndpoint activeEndpoint, long generation, long nativeConnection,
        long diagnosticEpoch
    ) {
        UsbRequest[] requests = new UsbRequest[HID_INPUT_REQUEST_DEPTH];
        try {
            for (int index = 0; index < requests.length; index++) {
                UsbRequest request = new UsbRequest();
                if (!request.initialize(activeConnection, activeEndpoint)) {
                    lastError = "Could not initialize QC HID input request " + (index + 1) + ".";
                    return;
                }
                ByteBuffer buffer = ByteBuffer.allocateDirect(QcNativeStateDecoder.REPORT_SIZE);
                request.setClientData(buffer);
                requests[index] = request;
            }
            activeInputRequests = requests;
            for (int index = 0; index < requests.length; index++) {
                ByteBuffer buffer = (ByteBuffer) requests[index].getClientData();
                if (!requests[index].queue(buffer)) {
                    lastError = "Could not queue QC HID input request " + (index + 1) + ".";
                    return;
                }
            }
            while (readerIsActive(activeConnection, generation)) {
                readerWaiting = true;
                UsbRequest completed = null;
                while (completed == null && readerIsActive(activeConnection, generation)) {
                    readAttempts++;
                    try { completed = activeConnection.requestWait(); }
                    finally { readerWaiting = false; }
                }
                long receivedAtMs = monotonicMillis();
                long reportEpoch = stateDecoder.captureReportEpoch(nativeConnection);
                if (!readerIsActive(activeConnection, generation)) return;
                ByteBuffer buffer = (ByteBuffer) completed.getClientData();
                if (buffer == null) {
                    lastError = "The QC HID input request ended without a matching completion.";
                    return;
                }
                int count = buffer.position();
                if (count > 0) {
                    buffer.flip();
                    byte[] bytes = new byte[count];
                    buffer.get(bytes);
                    consumeInputReport(bytes, count, nativeConnection, reportEpoch, diagnosticEpoch, receivedAtMs, generation);
                }
                buffer.clear();
                if (!completed.queue(buffer)) {
                    lastError = "Could not requeue a QC HID input request.";
                    return;
                }
            }
        } catch (Exception error) {
            if (readerIsActive(activeConnection, generation)) {
                lastReaderError = error.getClass().getName() + ": " + error.getMessage();
                lastError = "QC HID reader stopped: " + lastReaderError;
                android.util.Log.e("QcUsbPlugin", lastError, error);
            }
        } finally {
            boolean recoverReader = readerIsActive(activeConnection, generation);
            if (recoverReader) readerTerminatedGeneration = generation;
            readerWaiting = false;
            readerExitedAt = System.currentTimeMillis();
            if (activeInputRequests == requests) activeInputRequests = null;
            for (UsbRequest request : requests) if (request != null) {
                try { request.cancel(); } catch (Exception ignored) {}
                try { request.close(); } catch (Exception ignored) {}
            }
            if (recoverReader) recoverUnexpectedReaderExit(activeConnection, generation);
        }
    }

    private synchronized void recoverUnexpectedReaderExit(
        UsbDeviceConnection activeConnection, long generation
    ) {
        if (!readerIsActive(activeConnection, generation)) return;
        if (flight != null) flight.event("reader-exited-unexpectedly");
        if (stateDecoder.sessionTerminalReadFailed()) {
            terminalTransportFailed(
                "QC HID reader terminated; reconnecting the transport.",
                "hid-reader-terminal");
        }
    }

    private synchronized void scheduleSuspensionResync(long readerGeneration) {
        if (connection == null || readerGeneration != connectionGeneration.get()
                || relayAdmissionGuard.isTerminal()) return;
        String resync = stateDecoder.takeSuspensionResync();
        if (resync == null) return;
        final long intentGeneration = automaticReconnectGeneration.get();
        if (flight != null) flight.event("suspension-resync:" + resync);
        keepalive.execute(() -> {
            synchronized (this) {
                if (connection == null || readerGeneration != connectionGeneration.get()
                        || intentGeneration != automaticReconnectGeneration.get()
                        || relayAdmissionGuard.isTerminal()) return;
                relayReconnect("Quad Cortex resumed after " + resync
                    + "; resynchronizing the session", null, intentGeneration);
            }
        });
    }

    private void consumeInputReport(byte[] buffer, int count, long nativeConnection, long reportEpoch,
            long diagnosticEpoch, long receivedAtMs, long readerGeneration) {
        usbDiagnostics.rawReportReceived(diagnosticEpoch);
        // UsbRequest may complete with a residual one-byte read while a queued
        // ring is being cancelled for reconnect. It cannot contain the HID
        // length + flags header, so reject it before crossing JNI.
        if (count < 2 || count > buffer.length) {
            decodeErrors++;
            lastError = "Discarded truncated QC HID report of " + count + " byte(s).";
            android.util.Log.w("QcUsbPlugin", lastError);
            return;
        }
        QcPendingOperations.Entry<PendingBackup> activeBackup = pendingBackup;
        if (activeBackup != null) {
            activeBackup.operation.rawReports += 1;
            activeBackup.operation.lastRawReportBytes = count;
        }
        // The shared Rust transport accepts both Android's 128-byte HID body
        // and the 129-byte report-id-prefixed form used by desktop HID APIs.
        byte[] report = Arrays.copyOf(buffer, count);
        QcNativeStateDecoder.DecodedFrame decoded = decodeMessage(report, nativeConnection, reportEpoch, receivedAtMs);
        if (decoded == null) return;
        if (decoded.forwardingOnly) {
            synchronized (this) {
                if (reading && !transportClosing && stateDecoder.observationCurrent(decoded.observationTicket)) {
                    internetForwarder.accept(decoded.messageType, decoded.payload, decoded.observationTicket);
                } else {
                    Arrays.fill(decoded.payload, (byte) 0);
                }
            }
            return;
        }
        // A suspension that ended in a reset leaves every retained device state
        // suspect: rebuild the whole session, as a session reset does.
        scheduleSuspensionResync(readerGeneration);
        if (!decoded.states.isEmpty() || decoded.tempoClock != null) statePublicationDrain.request();
        // Lifecycle and state already changed atomically in nativePushReport.
        requestLifecycleDrain();
        if (!stateDecoder.observationCurrent(decoded.observationTicket)) return;
        if (activeBackup != null) activeBackup.operation.decodedMessages += 1;
        usbDiagnostics.messageReceived(diagnosticEpoch, decoded.messageType);
        if (flight != null) flight.inbound(decoded.messageType);
        lastMessageType = decoded.messageType;
        if (decoded.messageType == QcUsbProfile.MESSAGE_TYPE_RESET_COMMS_BUFFERS
            && decoded.lifecycle.sessionValidated && resetReply != null) {
            resetReply.countDown();
        }
        if (decoded.messageType == QcUsbProfile.MESSAGE_TYPE_GENERAL_SETTINGS
            || decoded.messageType == QcUsbProfile.MESSAGE_TYPE_IO_SETTINGS) {
            synchronized (stateEventLock) {
                if (stateDecoder.observationCurrent(decoded.observationTicket)) {
                    settingsEchoEvidence.observe(decoded.messageType, decoded.observationTicket, decoded.payload);
                }
            }
        }
        dispatchGatewayResponse(decoded.observationTicket, decoded.messageType, decoded.payload);
    }

    private QcNativeStateDecoder.DecodedFrame decodeMessage(byte[] report, long nativeConnection, long reportEpoch, long receivedAtMs) {
        try {
            QcNativeStateDecoder.DecodedFrame frame = stateDecoder.pushReport(nativeConnection, reportEpoch, report, receivedAtMs);
            if (frame == null) return null;
            int type = frame.messageType;
            byte[] payload = frame.payload;
            // ModelRepo expands on its own lane and installs atomically in the
            // same shared Rust decoder used for all realtime state.
            if (type == QcUsbProfile.MESSAGE_TYPE_MODEL_REPO) {
                scheduleModelCatalogDecode(payload, frame.observationTicket);
            }
            return frame;
        } catch (Exception error) {
            decodeErrors++;
            lastError = error.getMessage();
            android.util.Log.w("QcUsbPlugin", "Could not decode QC HID frame: " + lastError);
            statePublicationDrain.request();
            requestLifecycleDrain();
            return null;
        }
    }


    private void resolvePendingGatewayTransactions(long observationSequence, long now) {
        for (QcPendingOperations.Entry<PendingGatewayTransaction> entry : pendingOperations.entries(PendingGatewayTransaction.class)) {
            if (entry.result.isDone()) {
                pendingOperations.remove(entry);
                continue;
            }
            advancePendingGatewayTransaction(entry, observationSequence, now);
        }
    }

    private void advancePendingGatewayTransaction(
        QcPendingOperations.Entry<PendingGatewayTransaction> entry,
        long observationSequence, long now
    ) {
        PendingGatewayTransaction pending = entry.operation;
        if (entry.result.isDone() || !pending.dispatched) return;
        try {
            org.json.JSONObject action = stateDecoder.gatewayVerificationAdvance(
                pending.verificationId, pending.observationSequence(), now,
                pending.plan.method, pending.plan.verificationJson);
            switch (action.getString("kind")) {
                case "wait":
                    long delayMs = action.getLong("delayMs");
                    if (pending.wakeScheduled.compareAndSet(false, true)) {
                        keepalive.schedule(() -> {
                            pending.wakeScheduled.set(false);
                            long latestSequence;
                            synchronized (stateEventLock) { latestSequence = nextStateSequence - 1; }
                            advancePendingGatewayTransaction(
                                entry, latestSequence, monotonicMillis());
                        }, delayMs, TimeUnit.MILLISECONDS);
                    }
                    break;
                case "verified":
                    if (!pendingOperations.remove(entry)) return;
                    pending.wakeScheduled.set(false);
                    try {
                        org.json.JSONObject completion = QcGatewayCompletion.assemble(
                            action, pending.plan.method, pending.plan.detail, pending.plan.confirmationTimeoutMs,
                            pending.hostStartedAtUnixMs, pending.dispatchLatencyMs,
                            pending.observationSequence(), this::relaySnapshot);
                        entry.result.complete(completion);
                    } catch (Exception error) {
                        entry.result.completeExceptionally(error);
                    }
                    break;
                case "timedOut":
                    if (!pendingOperations.remove(entry)) return;
                    pending.wakeScheduled.set(false);
                    // The frames were delivered and the QC pushed no confirming
                    // state inside the window. That is not a failure and not an
                    // ambiguity: nothing is read, nothing is quarantined, and the
                    // result says exactly what is known. Same as the Windows broker.
                    try {
                        org.json.JSONObject completion = QcGatewayCompletion.assemble(
                            action, pending.plan.method, pending.plan.detail, pending.plan.confirmationTimeoutMs,
                            pending.hostStartedAtUnixMs, pending.dispatchLatencyMs,
                            0, this::relaySnapshot);
                        entry.result.complete(completion);
                    } catch (Exception error) {
                        entry.result.completeExceptionally(error);
                    }
                    break;
                default:
                    throw new IllegalStateException(
                        "Native QC verification returned an unknown action: " + action);
            }
        } catch (Exception error) {
            if (pendingOperations.remove(entry)) {
                stateDecoder.gatewayVerificationCancelled(pending.verificationId);
                entry.result.completeExceptionally(error);
            }
        }
    }

    /**
     * Reports a placed plugin block as read back, matching the desktop broker:
     * the native verifier attaches {@code pluginBlockReadback} only to a verified
     * {@code device.addBlock} of a plugin model, from the state it matched. When
     * that carries a note (the block came back bypassed without a bypass
     * request) the note is appended to {@code detail}. Ownership is the
     * device's to enforce, so nothing is refused or removed here.
     */
    static void attachPluginBlockReadback(
        org.json.JSONObject completion, org.json.JSONObject action
    ) throws org.json.JSONException {
        QcGatewayCompletion.attachPluginBlockReadback(completion, action);
    }

    static void applyNativeOutcome(
        org.json.JSONObject completion, org.json.JSONObject action
    ) throws org.json.JSONException {
        QcGatewayCompletion.applyNativeOutcome(completion, action);
    }

    private void resolvePendingCatalogRead(QcPendingOperations.Entry<PendingCatalogRead> entry) {
        if (entry.result.isDone()) {
            pendingOperations.remove(entry);
            return;
        }
        PendingCatalogRead pending = entry.operation;
        try {
            if (stateDecoder.requestExpired(pending.startedAtMs, pending.timeoutMs, monotonicMillis())) {
                throw new RelayException("READBACK_TIMEOUT", "The QC did not provide the requested catalog in time.");
            }
            org.json.JSONObject projection = stateDecoder.retainedCatalog(
                pending.context, pending.method, pending.key, monotonicMillis());
            stateDecoder.catalogContext(pending.context);
            if (stateDecoder.requestExpired(pending.startedAtMs, pending.timeoutMs, monotonicMillis())) {
                throw new RelayException("READBACK_TIMEOUT", "The QC catalog read expired before completion.");
            }
            if (projection.getBoolean("ready")) {
                org.json.JSONObject value = projection.getJSONObject("value");
                if (pendingOperations.remove(entry)) entry.result.complete(value);
            } else {
                // Bounded native-state polling also detects session replacement
                // when no further File frame arrives. This produces no USB I/O.
                long remainingMs = pending.timeoutMs - (monotonicMillis() - pending.startedAtMs);
                keepalive.schedule(() -> resolvePendingCatalogRead(entry),
                    Math.max(1L, Math.min(100L, remainingMs)), TimeUnit.MILLISECONDS);
            }
        } catch (Exception error) {
            if (pendingOperations.remove(entry)) entry.result.completeExceptionally(error);
        }
    }

    private void dispatchGatewayResponse(String observationTicket, int messageType, byte[] payload) {
        if (messageType == QcUsbProfile.MESSAGE_TYPE_LOCAL_BACKUP) {
            QcPendingOperations.Entry<PendingBackup> pending = pendingBackup;
            if (pending != null && !pending.operation.recoveryStarted) {
                try {
                    JSObject update = stateDecoder.consumeBackupChunk(
                        pending.operation.admission, observationTicket,
                        payload, pending.operation.name, monotonicMillis());
                    int chunks = update.getInteger("chunks", pending.operation.chunks);
                    int ignored = update.getInteger("ignoredPrefixChunks", pending.operation.ignoredPrefixChunks);
                    pending.operation.chunks = chunks;
                    pending.operation.ignoredPrefixChunks = ignored;
                    if (update.getBoolean("complete", false) && pendingBackup == pending) {
                        pending.operation.recoveryStarted = true;
                        org.json.JSONObject document = (org.json.JSONObject) update.get("backup");
                        backupFileIo.execute(() -> {
                            try {
                                pending.operation.savedResult = saveBackupDocument(document, pending.operation.name);
                                recoverSessionAfterBackup(pending);
                            } catch (Exception error) {
                                failPendingBackupRecovery(pending, error);
                            }
                        });
                    }
                } catch (Exception error) {
                    failPendingBackupRecovery(pending, error);
                }
            }
        }
        for (QcPendingOperations.Entry<PendingGatewayRead> entry : pendingOperations.entries(PendingGatewayRead.class)) {
            PendingGatewayRead pending = entry.operation;
            if (entry.result.isDone() || !gatewayReadEpochCurrent(entry)
                || usbDiagnostics.snapshot().messagesReceived <= pending.afterMessageSequence) continue;
            int response = stateDecoder.gatewayResponseDecision(
                pending.plan, messageType, payload, pending.startedAtMs, monotonicMillis());
            if (response == QcNativeStateDecoder.RESPONSE_EXPIRED) {
                if (pendingOperations.remove(entry)) entry.result.completeExceptionally(
                    new RelayException("READBACK_TIMEOUT", "The QC did not provide the requested reply in time."));
                continue;
            }
            if (response != QcNativeStateDecoder.RESPONSE_MATCHED) continue;
            try {
                readResponseIo.execute(() -> completeGatewayRead(entry, observationTicket, messageType, payload));
            } catch (java.util.concurrent.RejectedExecutionException error) {
                if (pendingOperations.remove(entry)) entry.result.completeExceptionally(
                    new RelayException("READBACK_BUSY", "The QC response decoder queue is full; the read was not completed."));
            } catch (Exception error) {
                lastGatewayReadMismatch = "type " + messageType + ": " + error.getMessage();
                // A response type may be shared by unrelated or differently
                // correlated device messages. Keep waiting for this plan's
                // shared Rust projection to accept the matching reply.
            }
        }
    }

    private void completeGatewayRead(QcPendingOperations.Entry<PendingGatewayRead> entry, String observationTicket, int messageType, byte[] payload) {
        if (entry.result.isDone() || !gatewayReadEpochCurrent(entry)) return;
        PendingGatewayRead pending = entry.operation;
        try {
            org.json.JSONObject observation = stateDecoder.observeGatewayRead(
                pending.plan, observationTicket, messageType, payload, pending.startedAtMs, monotonicMillis());
            switch (observation.getString("status")) {
                case "matched":
                    stateDecoder.gatewayReadScopeRemaining(pending.scope, monotonicMillis());
                    if (!gatewayReadEpochCurrent(entry)) return;
                    lastGatewayReadMismatch = null;
                    org.json.JSONObject value = observation.getJSONObject("value");
                    if (pendingOperations.remove(entry)) entry.result.complete(value);
                    break;
                case "expired":
                    if (pendingOperations.remove(entry)) entry.result.completeExceptionally(
                        new RelayException("READBACK_TIMEOUT", "The QC did not provide the requested reply in time."));
                    break;
                case "rejected":
                    lastGatewayReadMismatch = "type " + messageType + ": " + observation.getString("error");
                    break;
                case "unrelated": break;
                default: throw new IllegalStateException("Invalid native read observation.");
            }
        } catch (Exception error) {
            if (pendingOperations.remove(entry)) entry.result.completeExceptionally(error);
        }
    }

    private boolean gatewayReadEpochCurrent(QcPendingOperations.Entry<PendingGatewayRead> entry) {
        if (stateDecoder.readEpochCurrent(entry.operation.plan)) return true;
        if (pendingOperations.remove(entry)) entry.result.completeExceptionally(
            new RelayException("NOT_CONNECTED", "The QC session changed while awaiting the read."));
        return false;
    }

    private boolean statePublicationsReady() { return stateDecoder.statePublicationsReady(); }

    private void drainStatePublications() {
        long generation = connectionGeneration.get();
        try {
            for (QcNativeStateDecoder.StatePublication publication : stateDecoder.takeStatePublications()) {
                publishStateBatch(publication);
            }
        } catch (Exception error) {
            throw new StatePublicationFailure(generation, error);
        }
    }

    private void statePublicationFailed(Exception error) {
        synchronized (this) {
            if (error instanceof StatePublicationFailure
                && ((StatePublicationFailure) error).generation != connectionGeneration.get()) return;
            if (statePublicationIo.isShutdown() || connection == null) return;
            decodeErrors++;
            terminalTransportFailed("QC state publication failed: " + error.getMessage(), "state-publication-failed");
        }
    }

    private void publishStateBatch(QcNativeStateDecoder.StatePublication publication) {
        if (publication.states.isEmpty() && publication.tempoClock == null) return;
        long sequence;
        synchronized (stateEventLock) {
            if (!stateDecoder.observationCurrent(publication.observationTicket)) return;
            long observedAt = publication.observedAt;
            lastStateAt = observedAt;
            sequence = nextStateSequence++;
            for (QcPendingOperations.Entry<PendingGatewayTransaction> entry
                : pendingOperations.entries(PendingGatewayTransaction.class)) {
                if (stateDecoder.operationObservationIsFresh(entry.operation.admission, publication.observationTicket))
                    entry.operation.observeAdmitted(publication.observationFence, publication.states);
            }
            JSArray states = new JSArray();
            for (JSObject state : publication.states) {
                String kind = state.getString("kind", "");
                if ("master".equals(kind) && state.has("masterVolume")) {
                    double volume = state.optDouble("masterVolume", -1);
                    currentMasterVolume = volume < 0 ? -1 : (int) Math.round(volume * 100);
                    currentMasterVolumeObservedAt = observedAt;
                } else if ("position".equals(kind)) {
                    currentSetlist = state.getString("setlistKey", currentSetlist);
                    currentPosition = state.getInteger("position", currentPosition);
                }
                state.put("observedAt", observedAt);
                states.put(state);
            }
            JSObject frame = new JSObject();
            frame.put("streamId", stateStreamId);
            frame.put("observedAt", observedAt);
            frame.put("states", states);
            frame.put("sequence", sequence);
            JSObject tempoClock = publication.tempoClock;
            if (tempoClock != null) {
                frame.put("tempoClock", tempoClock);
                latestTempoClock = new JSObject()
                    .put("available", true)
                    .put("sequence", sequence)
                    .put("receivedAtUnixMs", observedAt)
                    .put("enabled", tempoClock.optBoolean("enabled"))
                    .put("prerollEnabled", tempoClock.optBoolean("prerollEnabled"))
                    .put("currentBeat", tempoClock.getInteger("currentBeat"))
                    .put("currentBar", tempoClock.getInteger("currentBar"))
                    .put("currentTick", tempoClock.getInteger("currentTick"))
                    .put("countInBeatsRemaining", tempoClock.getInteger("countInBeatsRemaining"))
                    .put("countInBarsRemaining", tempoClock.getInteger("countInBarsRemaining"));
            }
            boolean retained = stateEventLog.add(frame);
            // The bounded journal and shared cache recovery own reattachment.
            // Capacitor's retained argument list grows without a listener.
            if (retained) notifyListeners("qcStateBatch", frame, false);
        }
        // Completion callbacks can start other work; do not run them under the
        // publication/reset fence or while the native owner is locked.
        resolvePendingReady();
        resolvePendingGatewayTransactions(sequence, monotonicMillis());
    }

    private void recoverSessionAfterBackup(QcPendingOperations.Entry<PendingBackup> pending) {
        // Preserve Android's explicit post-save recovery policy. Carry the
        // completed attempt to the commandIo teardown boundary; file completion
        // alone cannot authorize restarting whichever connection exists later.
        synchronized (this) {
            if (pendingBackup != pending || pending.result.isDone() || pending.operation.savedResult == null) return;
            if (!pendingOperations.remove(pending)) return;
            pendingBackup = null;
        }
        org.json.JSONObject savedResult = pending.operation.savedResult;
        relayReconnect("USB session restored after device backup", pending.operation.admission).whenComplete((ignored, error) -> {
            if (error != null) {
                pending.result.completeExceptionally(new RelayException(
                    "USB_CONNECT_FAILED",
                    "The backup was saved, but the Quad Cortex session did not recover: "
                        + (error.getMessage() == null ? error.toString() : error.getMessage())));
            } else {
                pending.result.complete(savedResult);
            }
        });
    }

    private void failPendingBackupRecovery(
        QcPendingOperations.Entry<PendingBackup> pending, Exception error
    ) {
        if (pendingBackup == pending) {
            pendingBackup = null;
            stateDecoder.backupCancelled(pending.operation.admission);
        }
        if (pendingOperations.remove(pending)) pending.result.completeExceptionally(error);
    }

    private void scheduleModelCatalogDecode(byte[] payload, String observationTicket) {
        if (stateDecoder.queueModelRepo(observationTicket, payload)) requestMetadataDrain();
    }

    private void requestMetadataDrain() {
        if (!metadataDrainScheduled.compareAndSet(false, true)) return;
        try {
            metadataIo.execute(() -> {
                try {
                    while (true) {
                        QcNativeStateDecoder.MetadataResult result = stateDecoder.parseNextModelRepo(monotonicMillis());
                        if (result == null) break;
                        requestLifecycleDrain();
                        if (result.error != null) {
                            decodeErrors++;
                            lastError = result.error;
                        }
                        statePublicationDrain.request();
                    }
                } catch (Exception error) {
                    decodeErrors++;
                    lastError = error.getMessage();
                } finally {
                    statePublicationDrain.request();
                    metadataDrainScheduled.set(false);
                    // Recheck after releasing the wake flag: an ingress event
                    // that raced the last empty pull must not lose its wakeup.
                    if (!metadataIo.isShutdown() && stateDecoder.modelRepoReady()) requestMetadataDrain();
                    requestLifecycleDrain();
                }
            });
        } catch (java.util.concurrent.RejectedExecutionException error) {
            metadataDrainScheduled.set(false);
            if (!metadataIo.isShutdown()) {
                decodeErrors++;
                lastError = "QC metadata executor rejected its coalesced wakeup";
            }
        }
    }

    private UsbDevice findQuadCortex() {
        for (UsbDevice candidate : manager.getDeviceList().values()) if (QcUsbDevicePolicy.isQuadCortex(candidate)) return candidate;
        return null;
    }

    private synchronized void closeConnection() {
        closeConnection(null);
    }

    private synchronized void closeConnection(QcPendingOperations.Entry<?> preservedOperation) {
        closeConnection(preservedOperation, true);
    }

    private synchronized void closeConnection(
        QcPendingOperations.Entry<?> preservedOperation, boolean sendDisconnectCommands
    ) {
        transportClosing = true;
        internetForwarder.cancelAll();
        boolean hadSession = connection != null || reading;
        if (flight != null && hadSession) flight.event("transport-closing");
        if (sendDisconnectCommands && connection != null && stateDecoder.startupActive()) {
            try {
                // Enter the Disconnected state the same way before the
                // transport is torn down: CPULoad DELETE, then Connection false.
                writeMessages(stateDecoder.disconnectCommands());
            } catch (Exception ignored) {
                // Teardown must remain unconditional if the link is already bad.
            }
        }
        synchronized (stateEventLock) {
            // Native invalidation and publication share this host fence. A
            // previously drained batch cannot append or notify after reset.
            stateDecoder.sessionDisconnected(monotonicMillis());
            stateDecoder.reset();
            currentSetlist = null;
            currentPosition = -1;
            currentMasterVolume = -1;
            currentMasterVolumeObservedAt = 0;
            stateEventLog.clear();
            latestTempoClock = null;
        }
        reading = false;
        commandNotBeforeMs = 0;
        connectionGeneration.incrementAndGet();
        UsbRequest[] inputRequests = activeInputRequests;
        if (inputRequests != null) {
            activeInputRequests = null;
            for (UsbRequest inputRequest : inputRequests) if (inputRequest != null) {
                try { inputRequest.cancel(); } catch (Exception ignored) {}
                try { inputRequest.close(); } catch (Exception ignored) {}
            }
        }
        if (midiConnection != null) {
            if (midiInterface != null) midiConnection.releaseInterface(midiInterface);
            midiConnection.close();
        }
        midiConnection = null;
        if (connection != null) {
            if (hidInterface != null) connection.releaseInterface(hidInterface);
            connection.close();
            lastClosedController = connection;
        }
        connection = null;
        device = null;
        hidInterface = null;
        inputEndpoint = null;
        midiInterface = null;
        midiOutputEndpoint = null;
        pendingOperations.failAllExcept(preservedOperation, () -> new RelayException(
            "NOT_CONNECTED", "Quad Cortex USB disconnected during a pending operation."));
        pendingBackup = null;
        if (pendingReady != preservedOperation) pendingReady = null;
        connectedAt = 0;
        if (flight != null && hadSession) flight.event("transport-closed");
    }

    @Override protected void handleOnPause() {
        demoProvider.foreground(false);
        if (!relaySessionAvailable()) brokerTargets.clear();
        if (!relayForegroundServiceEligible()) getContext().stopService(new Intent(getContext(), QcRelayService.class));
        super.handleOnPause();
    }

    @Override protected void handleOnResume() {
        demoProvider.foreground(true);
        startForegroundDemoRelay();
        super.handleOnResume();
    }

    private void startForegroundDemoRelay() {
        if (!demoProvider.available() || !new RelayCredentialStore(getContext()).paired()) return;
        try {
            getContext().startService(new Intent(getContext(), QcRelayService.class).setAction(QcRelayService.ACTION_START));
        } catch (RuntimeException unavailable) {
            // A foreground-to-background race must not crash the app. Resume
            // or the explicit Remote action can start this provider again.
            android.util.Log.w("QcUsbPlugin", "The foreground demo relay could not start.");
        }
    }

    @Override
    protected void handleOnDestroy() {
        demoProvider.publish(null, 0);
        automaticReconnectGeneration.incrementAndGet();
        internetForwarder.close();
        relayAdmissionGuard.markTerminal();
        rejectPendingConnect("QC Remote stopped before USB permission was answered.");
        statePublicationDrain.close();
        if (relaySession == this) relaySession = null;
        getContext().stopService(new Intent(getContext(), QcRelayService.class));
        // Scheduled workflow/readback callbacks may be discarded by shutdownNow.
        // Finish every owned source future first so those discarded callbacks
        // cannot retain a semaphore slot or this plugin session indefinitely.
        cancelRelayOperationsForSession(this);
        cancelDroppedRelayTasks(commandIo.shutdownNow());
        cancelDroppedRelayTasks(midiIo.shutdownNow());
        metadataIo.shutdownNow();
        statePublicationIo.shutdownNow();
        backupFileIo.shutdownNow();
        readResponseIo.shutdownNow();
        keepalive.shutdownNow();
        try { getContext().unregisterReceiver(permissionReceiver); } catch (Exception ignored) {}
        try { getContext().unregisterReceiver(deviceReceiver); } catch (Exception ignored) {}
        lifecycleTeardown.execute(() -> {
            try {
                closeConnection();
            } catch (Exception error) {
                android.util.Log.w("QcUsbPlugin", "USB teardown encountered an error.", error);
            } finally {
                QcUsbReaderTeardown.process().trackShutdown(readerIo);
                boolean readerStopped = false;
                try {
                    // The reader must stop before JNI destroys the decoder it
                    // may still be using. Keep this wait on the teardown lane.
                    readerStopped = QcUsbReaderTeardown.process().await(Long.MAX_VALUE);
                } catch (InterruptedException error) {
                    Thread.currentThread().interrupt();
                }
                boolean workersStopped = true;
                workersStopped &= awaitExecutor(commandIo);
                workersStopped &= awaitExecutor(midiIo);
                workersStopped &= awaitExecutor(metadataIo);
                workersStopped &= awaitExecutor(statePublicationIo);
                workersStopped &= awaitExecutor(backupFileIo);
                workersStopped &= awaitExecutor(readResponseIo);
                workersStopped &= awaitExecutor(keepalive);
                try { workersStopped &= internetForwarder.awaitTermination(); }
                catch (InterruptedException error) { Thread.currentThread().interrupt(); workersStopped = false; }
                readerStopped = readerStopped && workersStopped;
                if (readerStopped) {
                    try { stateDecoder.close(); }
                    catch (Exception error) {
                        android.util.Log.w("QcUsbPlugin", "Could not close the native QC state decoder.", error);
                    }
                } else {
                    android.util.Log.w("QcUsbPlugin", "Reader did not stop; leaving its native decoder alive to avoid a use-after-free.");
                }
                if (flight != null) flight.close();
            }
        });
        lifecycleTeardown.shutdown();
        super.handleOnDestroy();
    }

    private static boolean awaitExecutor(ExecutorService executor) {
        try { return executor.awaitTermination(Long.MAX_VALUE, TimeUnit.NANOSECONDS); }
        catch (InterruptedException error) {
            Thread.currentThread().interrupt();
            return false;
        }
    }
}
