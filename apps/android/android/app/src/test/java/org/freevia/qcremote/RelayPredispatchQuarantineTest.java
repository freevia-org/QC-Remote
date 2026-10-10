package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.nio.file.*;
import java.util.Comparator;
import org.json.JSONObject;
import org.junit.Test;

/** Runs verbatim production admission/catch/serialization; native dispatch is the counted boundary. */
public class RelayPredispatchQuarantineTest {
    private static String slice(String source, String from, String to) {
        int start = source.indexOf(from);
        int end = source.indexOf(to, start + from.length());
        assertTrue("production boundary: " + from, start >= 0 && end > start);
        assertEquals("unique production boundary", -1, source.indexOf(from, start + 1));
        return source.substring(start, end);
    }

    private static Path findPlugin() {
        for (Path root = Paths.get(System.getProperty("user.dir")).toAbsolutePath(); root != null; root = root.getParent()) {
            for (String relative : new String[]{"src/main/java/org/freevia/qcremote/QcUsbPlugin.java",
                "app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java",
                "apps/android/android/app/src/main/java/org/freevia/qcremote/QcUsbPlugin.java"}) {
                Path path = root.resolve(relative);
                if (Files.isRegularFile(path)) return path;
            }
        }
        throw new AssertionError("Production plugin not found");
    }

    @Test public void actualPredispatchBranchSendsNothingAndPreservesCarrier() throws Exception {
        Path pluginPath = findPlugin();
        String plugin = new String(Files.readAllBytes(pluginPath), StandardCharsets.UTF_8);
        String prefix = slice(plugin,
            "    private CompletableFuture<org.json.JSONObject> relayInvoke(",
            "            if (\"SYSTEM\".equals(dispatch))");
        String catches = slice(plugin,
            "        } catch (RelayException error) {",
            "    private static boolean isMutationDispatch(");
        String mutation = slice(plugin, "    private static boolean isMutationDispatch(",
            "    private CompletableFuture<org.json.JSONObject> relayPlannedGatewayWrite(");
        String failed = slice(plugin,
            "    private static CompletableFuture<org.json.JSONObject> failedRelay(String code, String message)",
            "    private CompletableFuture<org.json.JSONObject> relayDisconnect()");
        String exception = new String(Files.readAllBytes(pluginPath.resolveSibling("RelayException.java")), StandardCharsets.UTF_8)
            .replace("package org.freevia.qcremote;", "");
        String protocol = new String(Files.readAllBytes(pluginPath.resolveSibling("RelayProtocol.java")), StandardCharsets.UTF_8)
            .replace("package org.freevia.qcremote;", "").replace("import org.json.JSONObject;", "");
        String generated = new String(Files.readAllBytes(pluginPath.resolveSibling("GeneratedGatewayMethods.java")), StandardCharsets.UTF_8);
        String dispatch = slice(generated, "    static String dispatchKind(String method)", "    static String resultKind(String method)");
        String java = HARNESS + prefix
            + "            submissions++; return CompletableFuture.completedFuture(new JSONObject());\n"
            + catches + mutation + failed + "}\n" + exception + protocol + DEPENDENCIES
            + "class GeneratedGatewayMethods {static boolean contains(String s){return true;} static void validateParams(String s,JSONObject p){}"
            + dispatch + "}";
        Path output = Files.createTempDirectory("qc-predispatch-quarantine-");
        URLClassLoader loader = null;
        try {
            Path source = output.resolve("AdmissionHarness.java");
            Files.write(source, java.getBytes(StandardCharsets.UTF_8));
            Object compiler = Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
            assertNotNull(compiler);
            String jsonClasspath = Paths.get(JSONObject.class.getProtectionDomain().getCodeSource().getLocation().toURI()).toString();
            try (java.io.ByteArrayOutputStream diagnostics = new java.io.ByteArrayOutputStream()) {
                String[] args = {"-classpath", jsonClasspath, "-d", output.toString(), "-proc:none", source.toString()};
                int code = (Integer) Class.forName("javax.tools.Tool").getMethod("run",
                    java.io.InputStream.class, java.io.OutputStream.class, java.io.OutputStream.class, String[].class)
                    .invoke(compiler, null, diagnostics, diagnostics, args);
                assertEquals(diagnostics.toString("UTF-8"), 0, code);
            }
            loader = new URLClassLoader(new java.net.URL[]{output.toUri().toURL()}, getClass().getClassLoader());
            Class<?> harness = Class.forName("AdmissionHarness", true, loader);
            Object value = harness.getConstructor().newInstance();
            for (boolean throughCatch : new boolean[]{false, true}) {
                JSONObject result = (JSONObject) harness.getMethod("run", boolean.class, boolean.class)
                    .invoke(value, !throughCatch, throughCatch);
                assertEquals(0, result.getInt("submissions"));
                JSONObject error = result.getJSONObject("wire").getJSONObject("error");
                assertEquals("WRITE_QUARANTINED", error.getString("code"));
                assertFalse(error.getBoolean("retryable"));
                assertFalse(error.has("userActionRequired"));
                assertFalse(error.getJSONObject("details").getBoolean("requestSent"));
                assertTrue(error.getJSONObject("details").getBoolean("writesQuarantined"));
            }
            JSONObject admitted = (JSONObject) harness.getMethod("run", boolean.class, boolean.class)
                .invoke(value, false, false);
            assertEquals("control proves downstream boundary is reachable", 1, admitted.getInt("submissions"));
        } finally {
            try { if (loader != null) loader.close(); }
            finally {
                try (java.util.stream.Stream<Path> paths = Files.walk(output)) {
                    for (Path path : (Iterable<Path>) paths.sorted(Comparator.reverseOrder())::iterator) Files.delete(path);
                }
                assertFalse("owned compiler fixture removed", Files.exists(output));
            }
        }
    }

    private static final String HARNESS = "import org.json.JSONObject; import java.util.concurrent.*;\n"
        + "public class AdmissionHarness { public int submissions; Guard relayAdmissionGuard = new Guard(); Decoder stateDecoder = new Decoder(); Flight flight;\n"
        + "public JSONObject run(boolean quarantine, boolean throwThroughCatch) throws Exception { submissions=0; relayAdmissionGuard.value=quarantine; stateDecoder.throwThroughCatch=throwThroughCatch;"
        + "try { relayInvoke(\"device.setTempo\",new JSONObject(),new JSONObject()).join(); return new JSONObject().put(\"submissions\",submissions); }"
        + "catch(CompletionException failure) { return new JSONObject().put(\"submissions\",submissions).put(\"wire\",RelayProtocol.error(\"fixture\",(RelayException)failure.getCause())); } }\n";
    private static final String DEPENDENCIES = "\nclass Guard { boolean value; boolean writesQuarantined(){return value;} }"
        + "class Decoder { boolean throwThroughCatch; JSONObject mergeExpectedState(JSONObject p,JSONObject e){if(throwThroughCatch)throw RelayException.quarantinedBeforeDispatch();return p;} }"
        + "class Flight {void activity(String ignored){}}"
        + "class JSObject {static JSONObject fromJSONObject(JSONObject value){return value;}}"
        + "class GeneratedRelayProfile {static final String PROTOCOL_VERSION=\"fixture\";}"
        + "class GeneratedRemoteActions {static boolean contains(String s){return true;}static boolean isReadOnly(String s){return false;}static boolean requiresConfirmation(String s){return true;}}";
}
