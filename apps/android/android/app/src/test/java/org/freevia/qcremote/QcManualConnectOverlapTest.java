package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.Comparator;
import org.junit.*;

/** Exercises the production Connect methods with a controlled native recovery future. */
public final class QcManualConnectOverlapTest {
    private static Path output;
    private static URLClassLoader loader;
    private static Class<?> harness;

    private static String between(String source, String from, String until) {
        int start = source.indexOf(from), end = source.indexOf(until, start);
        assertTrue("production method boundary", start >= 0 && end > start);
        return source.substring(start, end);
    }

    @BeforeClass public static void compileProductionPaths() throws Exception {
        Path plugin = null;
        for (Path root = Paths.get(System.getProperty("user.dir")).toAbsolutePath(); root != null && plugin == null; root = root.getParent()) {
            for (String relative : new String[] {"src/main/java/org/freevia/qcremote/QcUsbPlugin.java", "app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java", "apps/android/android/app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java"}) {
                Path candidate = root.resolve(relative);
                if (Files.isRegularFile(candidate)) { plugin = candidate; break; }
            }
        }
        assertNotNull("this checkout's production source", plugin);
        String source = new String(Files.readAllBytes(plugin), StandardCharsets.UTF_8).replace("\r\n", "\n");
        String connect = between(source, "    @PluginMethod\n    public void connect(PluginCall call)", "    /** A deliberate Connect shares");
        String await = between(source, "    private void awaitReconnectForConnect(", "    /** Release a `connecting` flag");
        String stale = between(source, "    private boolean clearStaleConnectingLatch()", "    /** Refuse an outstanding prompt");
        String open = between(source, "    private void openAndHandshake(UsbDevice candidate, PluginCall call)", "    private void openDeviceAndHandshake(");
        output = Files.createTempDirectory("qc-manual-connect-overlap-");
        boolean complete = false;
        try {
            Path java = output.resolve("ManualConnectHarness.java");
            Files.write(java, (PREFIX + connect + await + stale + open + SUFFIX).getBytes(StandardCharsets.UTF_8));
            Object compiler = Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
            assertNotNull("host JDK compiler", compiler);
            try (java.io.ByteArrayOutputStream diagnostics = new java.io.ByteArrayOutputStream()) {
                String[] args = {"-d", output.toString(), "-proc:none", java.toString()};
                int status = (Integer) Class.forName("javax.tools.Tool").getMethod("run", java.io.InputStream.class, java.io.OutputStream.class, java.io.OutputStream.class, String[].class)
                    .invoke(compiler, null, diagnostics, diagnostics, args);
                assertEquals(diagnostics.toString("UTF-8"), 0, status);
            }
            loader = new URLClassLoader(new java.net.URL[] {output.toUri().toURL()}, QcManualConnectOverlapTest.class.getClassLoader());
            harness = Class.forName("ManualConnectHarness", true, loader);
            complete = true;
        } finally { if (!complete) cleanup(); }
    }

    private void scenario(String name) throws Exception {
        try { harness.getMethod(name).invoke(harness.getConstructor().newInstance()); }
        catch (java.lang.reflect.InvocationTargetException error) {
            if (error.getCause() instanceof AssertionError) throw (AssertionError) error.getCause();
            throw error;
        }
    }

    @Test public void manualConnectWaitsForNativeRecovery() throws Exception { scenario("joinsRecovery"); }
    @Test public void nativeReadyTimeoutSettlesManualConnect() throws Exception { scenario("recoveryTimeout"); }
    @Test public void reconnectStartingBeforeLatchGuardIsJoined() throws Exception { scenario("beforeLatchGuard"); }
    @Test public void reconnectStartingBetweenGuardsIsJoined() throws Exception { scenario("racingRecovery"); }
    @Test public void oldReadySessionDoesNotBypassReplacement() throws Exception { scenario("oldReadySession"); }
    @Test public void replacedDeviceCannotSatisfyManualConnect() throws Exception { scenario("replacedDevice"); }
    @Test public void ordinaryManualConnectStillOpensOnce() throws Exception { scenario("ordinaryOpen"); }

    @AfterClass public static void cleanup() throws Exception {
        try { if (loader != null) loader.close(); }
        finally {
            loader = null;
            if (output != null) {
                try (java.util.stream.Stream<Path> paths = Files.walk(output)) {
                    for (Path path : (Iterable<Path>) paths.sorted(Comparator.reverseOrder())::iterator) Files.delete(path);
                }
                assertFalse("compiler staging removed", Files.exists(output)); output = null;
            }
        }
    }

    private static final String PREFIX = """
import java.util.*; import java.util.concurrent.*; import java.util.concurrent.atomic.*;
public final class ManualConnectHarness {
  volatile boolean connecting, ready, synchronizedSession;
  volatile long connectingSinceMs; volatile CompletableFuture<org.json.JSONObject> reconnectInFlight;
  final UsbDevice candidate = new UsbDevice(1), replacement = new UsbDevice(2);
  UsbDevice device; Object currentSetlist; PluginCall pendingConnect; String lastError;
  final Manager manager = new Manager(); final Decoder stateDecoder = new Decoder();
  final AtomicLong automaticReconnectGeneration = new AtomicLong();
  final Guard relayAdmissionGuard = new Guard();
  final Object commandIo = new Object(); final ArrayDeque<Runnable> queued = new ArrayDeque<>();
  final ThreadLocal<Object> CURRENT_RELAY_DEADLINE = new ThreadLocal<>();
  final Flight flight = null; final String USB_PERMISSION = "permission";
  long now; Runnable onReadyCheck;
  long monotonicMillis() { return now; }
  boolean isReady() { if (onReadyCheck != null) { Runnable action = onReadyCheck; onReadyCheck = null; action.run(); } return ready; }
  UsbDevice findQuadCortex() { return candidate; }
  void resolveConnected(PluginCall call, UsbDevice selected) { call.resolve(selected); }
  void closeConnection() { ready = false; device = null; }
  void openDeviceAndHandshake(UsbDevice selected, Object ignored, long epoch, long intent) {
    device = selected; ready = true; synchronizedSession = true; currentSetlist = new Object();
  }
  void submitRelayTask(Object executor, Object deadline, Runnable action) { queued.add(action); }
  Throwable unwrapCompletion(Throwable error) { while (error instanceof CompletionException && error.getCause() != null) error = error.getCause(); return error; }
  Context getContext() { return new Context(); }
  void check(boolean value, String message) { if (!value) throw new AssertionError(message); }
""";

    private static final String SUFFIX = """
  public void joinsRecovery() {
    connecting = true; reconnectInFlight = new CompletableFuture<>(); PluginCall call = new PluginCall();
    connect(call);
    check(!call.resolved && call.code == null && queued.isEmpty(), "manual Connect must wait for the existing open");
    device = candidate; ready = true; synchronizedSession = true; currentSetlist = new Object();
    reconnectInFlight.complete(new org.json.JSONObject());
    check(call.resolved && call.code == null && call.selected == candidate, "ready recovery did not settle manual Connect");
  }
  public void recoveryTimeout() {
    connecting = true; reconnectInFlight = new CompletableFuture<>(); PluginCall call = new PluginCall();
    connect(call); reconnectInFlight.completeExceptionally(new RelayException("READBACK_TIMEOUT", "The Quad Cortex did not finish synchronizing in time."));
    check(!call.resolved && "READBACK_TIMEOUT".equals(call.code), "ready timeout did not settle manual Connect");
  }
  public void beforeLatchGuard() {
    CompletableFuture<org.json.JSONObject> recovery = new CompletableFuture<>(); PluginCall call = new PluginCall();
    onReadyCheck = () -> { connecting = true; reconnectInFlight = recovery; };
    connect(call);
    check(!call.resolved && call.code == null && queued.isEmpty(), "recovery starting before latch guard was rejected");
    device = candidate; ready = true; synchronizedSession = true; currentSetlist = new Object(); recovery.complete(new org.json.JSONObject());
    check(call.resolved && call.code == null, "late recovery did not settle manual Connect");
  }
  public void racingRecovery() {
    CompletableFuture<org.json.JSONObject> recovery = new CompletableFuture<>(); PluginCall call = new PluginCall();
    manager.onPermissionCheck = () -> { connecting = true; reconnectInFlight = recovery; };
    connect(call);
    check(!call.resolved && call.code == null && queued.isEmpty(), "racing recovery was rejected or duplicated");
    device = candidate; ready = true; synchronizedSession = true; currentSetlist = new Object(); recovery.complete(new org.json.JSONObject());
    check(call.resolved && call.code == null, "racing recovery did not settle manual Connect");
  }
  public void oldReadySession() {
    device = candidate; ready = true; synchronizedSession = true; currentSetlist = new Object();
    connecting = true; reconnectInFlight = new CompletableFuture<>(); PluginCall call = new PluginCall(); connect(call);
    check(!call.resolved && call.code == null, "old ready session bypassed pending replacement");
    reconnectInFlight.completeExceptionally(new RelayException("READBACK_TIMEOUT", "Replacement did not synchronize."));
    check(!call.resolved && "READBACK_TIMEOUT".equals(call.code), "failed replacement was reported as old ready session");
  }
  public void replacedDevice() {
    connecting = true; reconnectInFlight = new CompletableFuture<>(); PluginCall call = new PluginCall(); connect(call);
    device = replacement; ready = true; synchronizedSession = true; currentSetlist = new Object(); reconnectInFlight.complete(new org.json.JSONObject());
    check(!call.resolved && "USB_CONNECT_FAILED".equals(call.code), "a different USB device satisfied this Connect");
  }
  public void ordinaryOpen() {
    PluginCall call = new PluginCall(); connect(call);
    check(queued.size() == 1 && !call.resolved && call.code == null, "ordinary Connect did not queue one open");
    queued.remove().run();
    check(call.resolved && call.code == null && queued.isEmpty(), "ordinary Connect did not complete");
  }
  final class Manager { Runnable onPermissionCheck; boolean hasPermission(UsbDevice ignored) { if (onPermissionCheck != null) onPermissionCheck.run(); return true; } void requestPermission(UsbDevice ignored, PendingIntent intent) {} }
  final class Decoder { boolean sessionSynchronized() { return synchronizedSession; } }
  static final class Guard { long epoch() { return 1; } }
  static final class Flight { void event(String ignored) {} }
  static final class UsbDevice { final int id; UsbDevice(int id) { this.id = id; } int getDeviceId() { return id; } }
  static final class PluginCall { String code; boolean resolved; UsbDevice selected; org.json.JSONObject getData() { return new org.json.JSONObject(); } void resolve(UsbDevice device) { resolved = true; selected = device; } void reject(String message, String code) { this.code = code; } void reject(String message, String code, Exception ignored) { this.code = code; } }
  static final class RelayException extends RuntimeException { final String code; RelayException(String code, String message) { super(message); this.code = code; } }
  static final class Context { String getPackageName() { return "test"; } }
  static final class Intent { Intent(String ignored) {} Intent setPackage(String ignored) { return this; } }
  static final class PendingIntent { static final int FLAG_UPDATE_CURRENT = 1, FLAG_IMMUTABLE = 2; static PendingIntent getBroadcast(Context ignored, int request, Intent intent, int flags) { return new PendingIntent(); } }
  static final class QcUsbProfile { static final long READY_WAIT_TIMEOUT_MS = 35000; }
}
@interface PluginMethod {}
""";
}
