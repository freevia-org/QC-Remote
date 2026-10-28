package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.Comparator;
import org.junit.*;

/** Runs the production timer and deliberate-disconnect paths with a controlled scheduler. */
public final class QcAutomaticReconnectTest {
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
        String scheduling = between(source, "    private void scheduleAutomaticReconnect(String detail)", "    private void resolvePendingReady()");
        String reader = between(source, "    private synchronized void recoverUnexpectedReaderExit(", "    private void consumeInputReport(");
        String active = between(source, "    private boolean readerIsActive(", "    private boolean isDeviceStillAttached(");
        String terminal = between(source, "    private synchronized void terminalTransportFailed(", "    @RequiresApi(");
        String publication = between(source, "    private synchronized boolean publishConnectionAttempt(", "    private void resolveConnected(");
        String failureMessage = between(source, "    private static String connectionFailureMessage(Throwable error)", "    /** Refuse an outstanding prompt.");
        String disconnect = between(source, "    public void disconnect(PluginCall call)", "    @PluginMethod\n    public void diagnostics(");
        Object compiler = Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
        assertNotNull("host JDK compiler", compiler);
        output = Files.createTempDirectory("qc-automatic-reconnect-");
        boolean complete = false;
        try {
            Path java = output.resolve("ReconnectHarness.java");
            Files.write(java, (PREFIX + scheduling + disconnect + reader + active + terminal + publication + failureMessage + SUFFIX).getBytes(StandardCharsets.UTF_8));
            try (java.io.ByteArrayOutputStream diagnostics = new java.io.ByteArrayOutputStream()) {
                String[] args = {"-d", output.toString(), "-proc:none", java.toString()};
                int status = (Integer) Class.forName("javax.tools.Tool").getMethod("run", java.io.InputStream.class, java.io.OutputStream.class, java.io.OutputStream.class, String[].class)
                    .invoke(compiler, null, diagnostics, diagnostics, args);
                assertEquals(diagnostics.toString("UTF-8"), 0, status);
            }
            loader = new URLClassLoader(new java.net.URL[] {output.toUri().toURL()}, QcAutomaticReconnectTest.class.getClassLoader());
            harness = Class.forName("ReconnectHarness", true, loader);
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
    @Test public void explicitOpenCannotPublishAfterDisconnect() throws Exception { scenario("explicitPublicationAfterDisconnect"); }
    @Test public void deliberateDisconnectCancelsAlreadyQueuedRecovery() throws Exception { scenario("queuedDisconnect"); }
    @Test public void failedOldOpenCannotRearmAfterDisconnect() throws Exception { scenario("failedOpenAfterDisconnect"); }
    @Test public void currentRecoveryKeepsRetryingWhileDeviceIsAbsent() throws Exception { scenario("absentDeviceRetry"); }
    @Test public void openTransportWithoutSemanticSeedKeepsRetrying() throws Exception { scenario("unsynchronizedTransportRetry"); }
    @Test public void terminalReaderRecoveryCannotOverrideDisconnect() throws Exception { scenario("readerDisconnectRace"); }
    @Test public void replacementIntentAndTerminalOwnerRetireOldTimers() throws Exception { scenario("replacementAndShutdown"); }
    @Test public void currentSuspensionResyncReopensOnce() throws Exception { scenario("currentSuspensionResync"); }
    @Test public void retiredReaderCannotConsumeSuccessorResync() throws Exception { scenario("retiredSuspensionReader"); }
    @Test public void queuedSuspensionResyncCannotOverrideNewIntent() throws Exception { scenario("queuedSuspensionRetirement"); }
    @Test public void moduleStatisticsStartupFailureHasActionableConnectionStatus() throws Exception { scenario("moduleStatisticsFailureMessage"); }

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
import java.util.*; import java.util.concurrent.*; import java.util.concurrent.atomic.AtomicLong;
public final class ReconnectHarness {
 final AtomicLong automaticReconnectGeneration = new AtomicLong();
 String activeConnectionProgressRequestId; int connectionProgressAttempt;
 final Decoder stateDecoder = new Decoder(); final Guard relayAdmissionGuard = new Guard();
 final Scheduler keepalive = new Scheduler(); final Manager manager = new Manager();
 Flight flight; boolean connecting, present = true, reading = true, ready, synchronizedSession; int opens, closes; String lastError;
 Object currentSetlist;
 final AtomicLong connectionGeneration = new AtomicLong(1); UsbDeviceConnection connection = new UsbDeviceConnection();
 CompletableFuture<Object> openResult;
 static long monotonicMillis() { return 100; }
 boolean isReady() { return ready; }
 UsbDevice findQuadCortex() { return present ? new UsbDevice() : null; }
 CompletableFuture<Object> relayReconnect(String detail, String origin, Long generation) {
  if (generation != automaticReconnectGeneration.get()) throw new AssertionError("stale open");
  opens++; openResult = new CompletableFuture<>(); return openResult;
 }
 void closeConnection() { closes++; stateDecoder.due = false; reading = false; connection = null; connectionGeneration.incrementAndGet(); }
 void closeConnection(Object ignored, boolean graceful) { closeConnection(); }
 void check(boolean value, String message) { if (!value) throw new AssertionError(message); }
 void publishConnectionProgress(String requestId, String phase, String message, String errorCode) {}
 Throwable unwrapCompletion(Throwable error) { return error; }
 static final class RelayException extends RuntimeException { final String code = "TEST"; }
""";
    private static final String SUFFIX = """
 public void explicitPublicationAfterDisconnect() throws Exception {
  long intent = automaticReconnectGeneration.incrementAndGet();
  CountDownLatch claimed = new CountDownLatch(1), finishClaim = new CountDownLatch(1);
  boolean[] accepted = {true}; int[] publications = {0};
  Thread opener = new Thread(() -> {
   claimed.countDown();
   try { if (!finishClaim.await(2, TimeUnit.SECONDS)) throw new AssertionError("claim release missing"); }
   catch (InterruptedException error) { throw new AssertionError(error); }
   accepted[0] = publishConnectionAttempt(1, intent, () -> publications[0]++);
  });
  opener.start();
  try {
   check(claimed.await(2, TimeUnit.SECONDS), "explicit open never reached OS claim");
   disconnect(new PluginCall()); finishClaim.countDown(); opener.join(2000);
   check(!opener.isAlive() && !accepted[0] && publications[0] == 0, "old explicit open published after Disconnect");
   long fresh = automaticReconnectGeneration.incrementAndGet();
   check(publishConnectionAttempt(1, fresh, () -> publications[0]++) && publications[0] == 1, "fresh explicit attempt refused");
   check(!publishConnectionAttempt(0, fresh, () -> publications[0]++), "retired lifecycle accepted");
   relayAdmissionGuard.terminal = true;
   check(!publishConnectionAttempt(1, fresh, () -> publications[0]++), "terminal owner accepted");
  } finally { finishClaim.countDown(); opener.join(2000); }
 }
 public void readerDisconnectRace() throws Exception {
  UsbDeviceConnection old = connection;
  disconnect(new PluginCall()); recoverUnexpectedReaderExit(old, 1);
  check(stateDecoder.arms == 0 && keepalive.tasks.isEmpty(), "retired reader armed a new recovery");
  reading = true; connection = new UsbDeviceConnection(); long current = connectionGeneration.get();
  UsbDeviceConnection active = connection; stateDecoder.entered = new CountDownLatch(1); stateDecoder.release = new CountDownLatch(1);
  Thread reader = new Thread(() -> recoverUnexpectedReaderExit(active, current));
  Thread operator = new Thread(() -> disconnect(new PluginCall()));
  reader.start();
  try {
   check(stateDecoder.entered.await(2, TimeUnit.SECONDS), "reader did not enter terminal policy");
   operator.start(); stateDecoder.release.countDown(); reader.join(2000); operator.join(2000);
   check(!reader.isAlive() && !operator.isAlive(), "fixture threads did not retire");
   check(stateDecoder.arms == 1, "current loss should schedule before subsequent Disconnect cancels it");
   keepalive.next(); check(opens == 0 && keepalive.tasks.isEmpty(), "terminal callback borrowed newer Disconnect generation");
  } finally { stateDecoder.release.countDown(); reader.join(2000); if (operator.getState() != Thread.State.NEW) operator.join(2000); }
 }
 public void queuedDisconnect() {
  scheduleAutomaticReconnect("lost"); disconnect(new PluginCall()); keepalive.next();
  check(stateDecoder.arms == 1 && opens == 0 && keepalive.tasks.isEmpty(), "old timer rearmed after Disconnect");
 }
 public void moduleStatisticsFailureMessage() {
  String message = connectionFailureMessage(new RuntimeException("QC lifecycle failed: Protocol(ModuleStatisticsDataError)"));
  check(message.contains("USB opened") && message.contains("startup statistics") && message.contains("synchronization stopped"), "module-statistics failure was not explained in the connection status");
  check(message.contains("tap Retry") && message.contains("share Diagnostics"), "module-statistics status omitted next steps");
 }
 public void failedOpenAfterDisconnect() {
  scheduleAutomaticReconnect("lost"); keepalive.next(); check(opens == 1, "valid retry not opened");
  disconnect(new PluginCall()); openResult.completeExceptionally(new Exception("old open failed"));
  check(stateDecoder.arms == 1 && keepalive.tasks.isEmpty(), "old failed callback rearmed after Disconnect");
 }
 public void absentDeviceRetry() {
  present = false; scheduleAutomaticReconnect("lost"); keepalive.next();
  check(stateDecoder.arms == 2 && keepalive.tasks.size() == 1 && opens == 0, "valid recovery stopped before attachment");
  present = true; keepalive.next(); check(opens == 1 && keepalive.tasks.isEmpty(), "valid retry not opened after attachment");
 }
  public void unsynchronizedTransportRetry() {
   ready = true; scheduleAutomaticReconnect("semantic seed missing"); keepalive.next();
   check(opens == 1, "open transport incorrectly stopped semantic recovery");
   openResult.completeExceptionally(new Exception("ready wait timed out"));
   check(keepalive.tasks.size() == 1, "failed semantic recovery did not rearm");
   synchronizedSession = true; currentSetlist = new Object(); keepalive.next();
   check(opens == 1 && keepalive.tasks.isEmpty(), "fully synchronized session did not stop retrying");
  }
 public void replacementAndShutdown() {
  scheduleAutomaticReconnect("lost"); automaticReconnectGeneration.incrementAndGet(); keepalive.next();
  check(stateDecoder.arms == 1 && opens == 0, "old timer borrowed replacement intent");
  scheduleAutomaticReconnect("new loss"); relayAdmissionGuard.terminal = true; keepalive.next();
  check(stateDecoder.arms == 2 && opens == 0 && keepalive.tasks.isEmpty(), "terminal owner rearmed");
 }
 public void currentSuspensionResync() {
  stateDecoder.resync = "reset";
  scheduleSuspensionResync(connectionGeneration.get());
  check(keepalive.tasks.size() == 1 && opens == 0 && stateDecoder.resync == null, "current resync not queued");
  keepalive.next();
  check(opens == 1 && keepalive.tasks.isEmpty(), "current resync did not reopen once");
  scheduleSuspensionResync(connectionGeneration.get());
  check(keepalive.tasks.isEmpty(), "consumed resync queued twice");
 }
 public void retiredSuspensionReader() {
  long oldReader = connectionGeneration.get();
  disconnect(new PluginCall());
  connection = new UsbDeviceConnection(); reading = true; stateDecoder.resync = "successor";
  scheduleSuspensionResync(oldReader);
  check(keepalive.tasks.isEmpty() && "successor".equals(stateDecoder.resync), "old reader stole successor resync");
  scheduleSuspensionResync(connectionGeneration.get()); keepalive.next();
  check(opens == 1, "successor reader failed to reopen");
 }
 public void queuedSuspensionRetirement() {
  stateDecoder.resync = "reset"; scheduleSuspensionResync(connectionGeneration.get());
  disconnect(new PluginCall()); keepalive.next();
  check(opens == 0 && keepalive.tasks.isEmpty(), "queued resync overrode Disconnect");
  connection = new UsbDeviceConnection(); reading = true;
  stateDecoder.resync = "reset"; scheduleSuspensionResync(connectionGeneration.get());
  automaticReconnectGeneration.incrementAndGet(); keepalive.next();
  check(opens == 0 && keepalive.tasks.isEmpty(), "queued resync borrowed replacement intent");
  stateDecoder.resync = "reset"; scheduleSuspensionResync(connectionGeneration.get());
  connectionGeneration.incrementAndGet(); keepalive.next();
  check(opens == 0 && keepalive.tasks.isEmpty(), "queued resync borrowed replacement connection");
  stateDecoder.resync = "reset"; scheduleSuspensionResync(connectionGeneration.get());
  relayAdmissionGuard.terminal = true; keepalive.next();
  check(opens == 0 && keepalive.tasks.isEmpty(), "queued resync reopened terminal owner");
 }
 final class Decoder { int arms; boolean due; String resync; CountDownLatch entered, release; boolean sessionSynchronized() { return synchronizedSession; } String takeSuspensionResync() { String value = resync; resync = null; return value; } boolean sessionTerminalReadFailed() { if (entered != null) { entered.countDown(); try { if (!release.await(2, TimeUnit.SECONDS)) throw new AssertionError("terminal fixture timeout"); } catch (InterruptedException e) { Thread.currentThread().interrupt(); throw new AssertionError(e); } } return true; } long sessionScheduleReconnect(long now) { arms++; due = true; return 750; } boolean sessionReconnectDue(long now) { return due; } void sessionReconnectAttempted(long now) {} }
 static final class Guard { boolean terminal; boolean isTerminal() { return terminal; } boolean publish(long epoch, Runnable action) { if (terminal || epoch != 1) return false; action.run(); return true; } }
 static final class Scheduler { final ArrayDeque<Runnable> tasks = new ArrayDeque<>(); void schedule(Runnable task, long delay, TimeUnit unit) { tasks.add(task); } void execute(Runnable task) { tasks.add(task); } void next() { tasks.removeFirst().run(); } }
 static final class Manager { boolean hasPermission(UsbDevice device) { return true; } }
 static final class UsbDeviceConnection {} static final class UsbDevice {} static final class Flight { void event(String text) {} }
 static final class PluginCall { void resolve() {} }
}
@interface PluginMethod {}
class android { static class util { static class Log { static void w(String tag, String message) {} } } }
""";
}
