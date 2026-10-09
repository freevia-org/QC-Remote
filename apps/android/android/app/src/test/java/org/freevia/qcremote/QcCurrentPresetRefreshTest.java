package org.freevia.qcremote;

import static org.junit.Assert.*;

import java.lang.reflect.InvocationTargetException;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Comparator;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import org.json.JSONObject;
import org.junit.AfterClass;
import org.junit.BeforeClass;
import org.junit.Test;

/** Executes the production method verbatim, replacing only native state/USB dependencies. */
public final class QcCurrentPresetRefreshTest {
    private static Path output;
    private static URLClassLoader loader;
    private static Class<?> harness;
    private static Class<?> mappingHarness;

    @BeforeClass public static void compileProductionMethod() throws Exception {
        String source = new String(Files.readAllBytes(findPlugin()), StandardCharsets.UTF_8);
        String signature = "private CompletableFuture<org.json.JSONObject> relayCurrentPreset() throws Exception {";
        int start = source.indexOf(signature);
        int end = source.indexOf("    private static String firstText(", start);
        assertTrue("production refresh method must exist exactly once", start >= 0 && source.indexOf(signature, start + 1) < 0);
        assertTrue("production method boundary must exist", end > start);
        // Include the entire production method unchanged: no replacement of its guards or futures.
        String method = source.substring(start, end);
        int snapshotStart = source.indexOf("    private org.json.JSONObject relaySnapshot() throws Exception {");
        int snapshotEnd = source.indexOf("    private org.json.JSONObject decorateSnapshot(", snapshotStart);
        assertTrue("cache snapshot method boundary", snapshotStart >= 0 && snapshotEnd > snapshotStart);
        int dispatchStart = source.indexOf("                case \"SNAPSHOT\":", source.indexOf("private CompletableFuture<org.json.JSONObject> relayInvoke("));
        int dispatchEnd = source.indexOf("                case \"STATE_EVENTS\":", dispatchStart);
        assertTrue("production snapshot dispatch boundary", dispatchStart >= 0 && dispatchEnd > dispatchStart);
        // Execute the actual two dispatch branches unchanged. Native admission
        // and unrelated actions are outside this focused harness.
        String dispatch = " public CompletableFuture<JSONObject> dispatch(String dispatch) throws Exception { switch (dispatch) {\n"
            + source.substring(dispatchStart, dispatchEnd) + " default: throw new AssertionError(dispatch); } }\n";
        method += source.substring(snapshotStart, snapshotEnd) + dispatch;
        output = Files.createTempDirectory("qc-current-preset-method-");
        boolean ready = false;
        try {
            Path javaSource = output.resolve("RefreshHarness.java");
            Files.write(javaSource, (HARNESS_PREFIX + method + "\n}\n").getBytes(StandardCharsets.UTF_8));
            Path mappingSource = output.resolve("MappingHarness.java");
            Files.write(mappingSource, mappingSource(source).getBytes(StandardCharsets.UTF_8));
            // Android's compile API omits javax.tools; this JVM-only test accesses
            // the host JDK compiler reflectively rather than adding it to the app API.
            Object compiler = Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
            assertNotNull("Android JVM tests require a JDK compiler", compiler);
            String jsonClasspath = Paths.get(JSONObject.class.getProtectionDomain().getCodeSource().getLocation().toURI()).toString();
            try (java.io.ByteArrayOutputStream diagnostics = new java.io.ByteArrayOutputStream()) {
                String[] arguments = { "-classpath", jsonClasspath, "-d", output.toString(), "-proc:none", javaSource.toString(), mappingSource.toString() };
                int status = (Integer) Class.forName("javax.tools.Tool").getMethod("run",
                    java.io.InputStream.class, java.io.OutputStream.class, java.io.OutputStream.class, String[].class)
                    .invoke(compiler, null, diagnostics, diagnostics, arguments);
                assertEquals("verbatim production method must compile: " + diagnostics.toString("UTF-8"), 0, status);
            }
            loader = new URLClassLoader(new java.net.URL[] { output.toUri().toURL() }, QcCurrentPresetRefreshTest.class.getClassLoader());
            harness = Class.forName("RefreshHarness", true, loader);
            mappingHarness = Class.forName("MappingHarness", true, loader);
            ready = true;
        } finally {
            if (!ready) cleanup();
        }
    }

    private static String mappingSource(String plugin) throws Exception {
        Path directory = findPlugin().getParent();
        String decoder = new String(Files.readAllBytes(directory.resolve("QcNativeStateDecoder.java")), StandardCharsets.UTF_8);
        String exception = new String(Files.readAllBytes(directory.resolve("RelayException.java")), StandardCharsets.UTF_8)
            .replace("package org.freevia.qcremote;", "");
        int start = decoder.indexOf("    PlannedGatewayWrite gatewayPlan(");
        int end = decoder.indexOf("    boolean gatewayReadbackMatches(", start);
        assertTrue("native planner wrapper boundary", start >= 0 && end > start);
        int catchStart = plugin.indexOf("        } catch (RelayException error) {", plugin.indexOf("private CompletableFuture<org.json.JSONObject> relayInvoke("));
        int catchEnd = plugin.indexOf("    private static boolean isMutationDispatch(", catchStart);
        assertTrue("relayInvoke error boundary", catchStart >= 0 && catchEnd > catchStart);
        // Verbatim planner wrapper, RelayException, and relayInvoke catch chain.
        // Only the JNI planner and pre-catch dispatch boundary are replaced.
        return MAPPING_PREFIX + decoder.substring(start, end)
            + "  public CompletableFuture<JSONObject> invoke() { try { gatewayPlan(\"device.addBlock\", new JSObject()); submissions++; return CompletableFuture.completedFuture(new JSONObject());\n"
            + plugin.substring(catchStart, catchEnd) + "}\n" + exception;
    }

    @Test public void nativePlannerRefusalPreservesStateUnavailableWithoutSubmission() throws Exception {
        assertPlannerFailure("STATE_UNAVAILABLE: incomplete topology", "STATE_UNAVAILABLE", "incomplete topology");
    }

    @Test public void unrelatedNativeArgumentFailureRemainsInvalidArgumentWithoutSubmission() throws Exception {
        assertPlannerFailure("unknown argument", "INVALID_ARGUMENT", "unknown argument");
    }

    @SuppressWarnings("unchecked") private static void assertPlannerFailure(String nativeMessage, String code, String message) throws Exception {
        Object instance = mappingHarness.getConstructor(String.class).newInstance(nativeMessage);
        CompletableFuture<JSONObject> result = (CompletableFuture<JSONObject>) mappingHarness.getMethod("invoke").invoke(instance);
        assertTrue(result.isDone());
        try { result.join(); fail("expected native planner refusal"); }
        catch (CompletionException error) {
            Throwable cause = error.getCause();
            java.lang.reflect.Field field = cause.getClass().getDeclaredField("code");
            field.setAccessible(true);
            assertEquals(code, field.get(cause));
            assertEquals(message, cause.getMessage());
        }
        assertEquals("refusal must not reach the mocked submission boundary", 0, mappingHarness.getMethod("submissions").invoke(instance));
    }

    private static final String MAPPING_PREFIX =
        "import org.json.*; import java.util.concurrent.*;\n" +
        "public final class MappingHarness {\n" +
        " private final String nativeError; private int submissions;\n" +
        " public MappingHarness(String error) { nativeError = error; } public int submissions() { return submissions; }\n" +
        " static class JSObject extends JSONObject {} static class PlannedGatewayWrite { PlannedGatewayWrite(Object... values) {} }\n" +
        " long requireHandle() { return 1; }\n" +
        " String nativePlanGatewayWrite(long handle, String method, String args, long now) { throw new IllegalArgumentException(nativeError); }\n" +
        " long nonNegativeLong(JSONObject o, String key) { return o.getLong(key); }\n" +
        " String nullableString(JSONObject o, String key) { return o.optString(key, null); } Object encodedMessages(JSONArray a) { return a; }\n" +
        " CompletableFuture<JSONObject> failedRelay(String code, String message) { CompletableFuture<JSONObject> f = new CompletableFuture<>(); f.completeExceptionally(new RelayException(code,message)); return f; }\n" +
        " CompletableFuture<JSONObject> failedRelay(RelayException failure) { CompletableFuture<JSONObject> f = new CompletableFuture<>(); f.completeExceptionally(failure); return f; }\n";

    private static Path findPlugin() {
        for (Path root = Paths.get(System.getProperty("user.dir")).toAbsolutePath(); root != null; root = root.getParent()) {
            for (String relative : new String[] {
                "src/main/java/org/freevia/qcremote/QcUsbPlugin.java",
                "app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java",
                "apps/android/android/app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java"
            }) {
                Path candidate = root.resolve(relative);
                if (Files.isRegularFile(candidate)) return candidate;
            }
        }
        throw new AssertionError("Cannot locate this checkout's production QcUsbPlugin.java");
    }

    @AfterClass public static void cleanup() throws Exception {
        try { if (loader != null) loader.close(); }
        finally {
            loader = null;
            if (output != null) {
                try (java.util.stream.Stream<Path> paths = Files.walk(output)) {
                    for (Path path : (Iterable<Path>) paths.sorted(Comparator.reverseOrder())::iterator) Files.delete(path);
                }
                assertFalse("extracted compiler fixture must be removed", Files.exists(output));
                output = null;
            }
        }
    }

    private static final class Fixture {
        final Object instance;
        Fixture() throws Exception { instance = harness.getConstructor().newInstance(); }
        Object call(String method, Class<?>[] types, Object... args) throws Exception {
            try { return harness.getMethod(method, types).invoke(instance, args); }
            catch (InvocationTargetException error) {
                if (error.getCause() instanceof Exception) throw (Exception) error.getCause();
                throw error;
            }
        }
        @SuppressWarnings("unchecked") CompletableFuture<JSONObject> read() throws Exception {
            return (CompletableFuture<JSONObject>) call("read", new Class<?>[0]);
        }
        @SuppressWarnings("unchecked") CompletableFuture<JSONObject> dispatch(String name) throws Exception {
            return (CompletableFuture<JSONObject>) call("dispatch", new Class<?>[] { String.class }, name);
        }
        void publish(String session, long revision, long full, boolean complete) throws Exception {
            call("publish", new Class<?>[] { String.class, long.class, long.class, boolean.class }, session, revision, full, complete);
        }
        void action(String name) throws Exception { call(name, new Class<?>[0]); }
        int sends() throws Exception { return (Integer) call("sends", new Class<?>[0]); }
    }

    private static void rejected(CompletableFuture<?> future, String expected) {
        assertTrue("completion is deterministic, never wait on a timer", future.isDone());
        try { future.join(); fail("expected rejection: " + expected); }
        catch (CompletionException error) {
            Throwable cause = error;
            while (cause.getCause() != null) cause = cause.getCause();
            assertTrue(cause.toString(), cause.toString().contains(expected));
        }
    }

    @Test public void snapshotDispatchReturnsIncompleteCacheWithoutUsbSubmission() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> cached = f.dispatch("SNAPSHOT");
        assertTrue(cached.isDone());
        assertFalse(cached.join().getBoolean("gridLayoutComplete"));
        assertEquals(0, f.sends());
        assertEquals("cache reads must not enter explicit observation path", 0, f.call("observationReads", new Class<?>[0]));
    }

    @Test public void currentPresetDispatchRepairsOnceWhileSnapshotRemainsCacheOnly() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> repair = f.dispatch("CURRENT_PRESET");
        assertFalse(repair.isDone());
        assertEquals(1, f.sends());
        CompletableFuture<JSONObject> cached = f.dispatch("SNAPSHOT");
        assertTrue(cached.isDone());
        assertFalse(cached.join().getBoolean("gridLayoutComplete"));
        assertEquals(1, f.sends());
        f.publish("session-a", 2, 2, true);
        f.action("ack");
        assertTrue(repair.join().getBoolean("gridLayoutComplete"));
        assertEquals(1, f.sends());
    }

    @Test public void completeCacheReadsCopyWithoutSending() throws Exception {
        Fixture f = new Fixture();
        f.publish("session-a", 1, 1, true);
        JSONObject first = f.read().join();
        first.put("gridLayoutComplete", false);
        assertTrue(f.read().join().getBoolean("gridLayoutComplete"));
        assertEquals(0, f.sends());
    }

    @Test public void overlappingReadsShareOneSendAndIndependentResults() throws Exception {
        Fixture f = new Fixture();
        ExecutorService callers = Executors.newFixedThreadPool(2);
        CountDownLatch start = new CountDownLatch(1);
        CompletableFuture<JSONObject> first, second;
        try {
            Future<CompletableFuture<JSONObject>> a = callers.submit(() -> { start.await(); return f.read(); });
            Future<CompletableFuture<JSONObject>> b = callers.submit(() -> { start.await(); return f.read(); });
            start.countDown();
            first = a.get(5, TimeUnit.SECONDS);
            second = b.get(5, TimeUnit.SECONDS);
        } finally {
            start.countDown();
            callers.shutdownNow();
            assertTrue("fixture caller threads must retire", callers.awaitTermination(5, TimeUnit.SECONDS));
        }
        assertFalse(first.isDone());
        assertFalse(second.isDone());
        assertEquals(1, f.sends());
        f.publish("session-a", 2, 2, true);
        f.action("ack");
        assertTrue(first.join().getBoolean("gridLayoutComplete"));
        assertTrue(second.join().getBoolean("gridLayoutComplete"));
        assertNotSame(first.join(), second.join());
        first.join().put("gridLayoutComplete", false);
        assertTrue(second.join().getBoolean("gridLayoutComplete"));
        assertEquals(1, f.sends());
    }

    @Test public void unrelatedRevisionCannotReplaceFullPublication() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> read = f.read();
        f.publish("session-a", 25, 1, true);
        f.action("ack");
        rejected(read, "STATE_UNAVAILABLE");
        assertEquals(1, f.sends());
    }

    @Test public void newerFullCounterStillRequiresCompleteLayoutAndStateRevision() throws Exception {
        for (boolean complete : new boolean[] { false, true }) {
            Fixture f = new Fixture();
            CompletableFuture<JSONObject> read = f.read();
            f.publish("session-a", complete ? 1 : 2, 2, complete);
            f.action("ack");
            rejected(read, "STATE_UNAVAILABLE");
        }
    }

    @Test public void replacementSessionCannotSatisfyOldRead() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> read = f.read();
        f.publish("session-b", 30, 30, true);
        f.action("ack");
        rejected(read, "STATE_UNAVAILABLE");
    }

    @Test public void connectionGenerationFenceSurvivesSameSessionText() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> read = f.read();
        f.action("advanceConnection");
        f.publish("session-a", 2, 2, true);
        f.action("ack");
        rejected(read, "STATE_UNAVAILABLE");
    }

    @Test public void disconnectRejectsBothWaitersAndClearsSharedSlot() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> first = f.read(), second = f.read();
        f.action("disconnect");
        rejected(first, "NOT_CONNECTED");
        rejected(second, "NOT_CONNECTED");
        assertEquals(1, f.sends());
        f.publish("session-b", 1, 1, false);
        CompletableFuture<JSONObject> next = f.read();
        assertEquals(2, f.sends());
        f.publish("session-b", 2, 2, true);
        f.action("ack");
        assertTrue(next.join().getBoolean("gridLayoutComplete"));
    }

    @Test public void timeoutDoesNotRetryButAllowsLaterExplicitRead() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> first = f.read(), second = f.read();
        f.action("timeout");
        rejected(first, "READBACK_TIMEOUT");
        rejected(second, "READBACK_TIMEOUT");
        assertEquals(1, f.sends());
        CompletableFuture<JSONObject> next = f.read();
        assertEquals(2, f.sends());
        f.publish("session-a", 2, 2, true);
        f.action("ack");
        assertTrue(next.join().getBoolean("gridLayoutComplete"));
    }

    @Test public void laterCounterCannotCertifyAnOlderSnapshotSample() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> read = f.read();
        f.publish("session-a", 25, 1, true);
        f.action("advanceCounterAfterSample");
        f.action("ack");
        rejected(read, "STATE_UNAVAILABLE");
        assertEquals(2L, f.call("fullCounter", new Class<?>[0]));
        assertEquals(2, f.call("observationReads", new Class<?>[0]));
        assertEquals(0, f.call("splitReads", new Class<?>[0]));
        assertEquals(1, f.sends());
    }

    @Test public void initialObservationDoesNotSwallowConcurrentFullPublication() throws Exception {
        Fixture f = new Fixture();
        f.action("advanceCounterAfterSample");
        CompletableFuture<JSONObject> read = f.read();
        f.publish("session-a", 2, 2, true);
        f.action("ack");
        assertTrue(read.join().getBoolean("gridLayoutComplete"));
        assertEquals(2, f.call("observationReads", new Class<?>[0]));
        assertEquals(0, f.call("splitReads", new Class<?>[0]));
        assertEquals(1, f.sends());
    }

    @Test public void expiredScopeRejectsOtherwiseFreshCompletePublication() throws Exception {
        Fixture f = new Fixture();
        CompletableFuture<JSONObject> read = f.read();
        f.publish("session-a", 2, 2, true);
        f.action("expireScope");
        f.action("ack");
        rejected(read, "SCOPE_EXPIRED");
        assertEquals(1, f.sends());
    }

    // These are controllable boundaries, not a second refresh implementation.
    // The production method alone owns cache decisions, joining, fences and cleanup.
    private static final String HARNESS_PREFIX =
        "import java.util.concurrent.*; import java.util.concurrent.atomic.AtomicLong;\n" +
        "import org.json.*;\n" +
        "public final class RefreshHarness {\n" +
        "  private final Object currentPresetRefreshLock = new Object();\n" +
        "  private CompletableFuture<JSONObject> currentPresetGridRefresh;\n" +
        "  private final AtomicLong connectionGeneration = new AtomicLong(1);\n" +
        "  private static final ThreadLocal<RelayDeadline> CURRENT_RELAY_DEADLINE = new ThreadLocal<>();\n" +
        "  private final Decoder stateDecoder = new Decoder();\n" +
        "  private CompletableFuture<JSONObject> transport; private int sends;\n" +
        "  static class RelayDeadline {}\n" +
        "  static class RelayException extends RuntimeException { RelayException(String code, String message) { super(code + \": \" + message); } }\n" +
        "  static class Decoder {\n" +
        "    JSONObject snapshot; long full = 1; boolean expired, advanceAfterSample; int observations, splits;\n" +
        "    JSONObject snapshot() { return new JSONObject(snapshot.toString()); }\n" +
        "    void afterSample() { if (advanceAfterSample) { advanceAfterSample = false; full++; } }\n" +
        "    JSONObject currentPresetObservation() { observations++; JSONObject pair = new JSONObject().put(\"snapshot\", new JSONObject(snapshot.toString())).put(\"fullPresetRevision\", full); afterSample(); return pair; }\n" +
        "    JSONObject currentPreset() { splits++; JSONObject sampled = snapshot; afterSample(); return sampled; } long fullPresetRevision() { splits++; return full; }\n" +
        "    String beginGatewayReadScope(String method, JSONObject params, long now) { return \"scope\"; }\n" +
        "    long gatewayReadScopeRemaining(String scope, long now) { if (expired) throw new IllegalStateException(\"SCOPE_EXPIRED\"); return 1000; }\n" +
        "  }\n" +
        "  public RefreshHarness() { publish(\"session-a\", 1, 1, false); }\n" +
        "  public void publish(String session, long revision, long full, boolean complete) { stateDecoder.snapshot = new JSONObject().put(\"gridLayoutComplete\", complete).put(\"stateVersion\", new JSONObject().put(\"session\", session).put(\"revision\", revision)); stateDecoder.full = full; }\n" +
        "  private long monotonicMillis() { return 100; }\n" +
        "  private JSONObject decorateSnapshot(JSONObject value) { return value; }\n" +
        "  private CompletableFuture<JSONObject> failedRelay(String code, String message) { CompletableFuture<JSONObject> f = new CompletableFuture<>(); f.completeExceptionally(new RelayException(code, message)); return f; }\n" +
        "  private CompletableFuture<JSONObject> relayGatewayReadOnCurrentSession(String method, JSONObject params, String scope, RelayDeadline deadline) { if (!method.equals(\"device.currentPreset\")) throw new AssertionError(method); sends++; transport = new CompletableFuture<>(); return transport; }\n" +
        "  public CompletableFuture<JSONObject> read() throws Exception { return relayCurrentPreset(); }\n" +
        "  public int sends() { return sends; }\n" +
        "  public void ack() { transport.complete(new JSONObject()); }\n" +
        "  public void timeout() { transport.completeExceptionally(new RelayException(\"READBACK_TIMEOUT\", \"fixture timeout\")); }\n" +
        "  public void advanceConnection() { connectionGeneration.incrementAndGet(); }\n" +
        "  public void disconnect() { advanceConnection(); transport.completeExceptionally(new RelayException(\"NOT_CONNECTED\", \"fixture disconnect\")); }\n" +
        "  public void advanceCounterAfterSample() { stateDecoder.advanceAfterSample = true; }\n" +
        "  public long fullCounter() { return stateDecoder.full; } public int observationReads() { return stateDecoder.observations; } public int splitReads() { return stateDecoder.splits; }\n" +
        "  public void expireScope() { stateDecoder.expired = true; }\n";
}
