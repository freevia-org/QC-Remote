package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.nio.file.*;
import java.nio.charset.StandardCharsets;
import java.net.URLClassLoader;
import java.util.Comparator;
import org.json.JSONObject;
import org.junit.*;

/** Verbatim production pair callback and credential store; only transport/crypto/Android storage boundaries are mocked. */
public class RelayPairingFailureTest {
    private static Path output; private static URLClassLoader loader; private static Class<?> harness;
    @BeforeClass public static void compileActualProductionPaths() throws Exception {
        Path directory=findSource();
        String plugin=new String(Files.readAllBytes(directory.resolve("QcRelayPlugin.java")), StandardCharsets.UTF_8);
        String pair=between(plugin,"    private void pairWithCode(","    @PluginMethod public void start(");
        String response=between(plugin,"    static String pairingResponseError(","    static OkHttpClient pairingHttpClient(");
        String discard=between(plugin,"    static String discardForbiddenPendingRevocation(","    private void startService(");
        String endpointPolicy=new String(Files.readAllBytes(directory.resolve("RelayEndpointPolicy.java")), StandardCharsets.UTF_8)
            .replaceAll("(?m)^package .*;\\s*","").replaceAll("(?m)^import .*;\\s*","");
        String store=new String(Files.readAllBytes(directory.resolve("RelayCredentialStore.java")), StandardCharsets.UTF_8).replaceAll("(?m)^package .*;\\s*","").replaceAll("(?m)^import .*;\\s*","");
        int key=store.indexOf("    private SecretKey key()");assertTrue("production key boundary",key>=0);
        store=store.substring(0,key)+"    private Object key() { return new Object(); }\\n}\\n".replace("\\n","\n");
        String profile=new String(Files.readAllBytes(directory.resolve("GeneratedRelayProfile.java")), StandardCharsets.UTF_8).replace("package org.freevia.qcremote;","");
        output=Files.createTempDirectory("qc-pairing-failure-");boolean complete=false;
        try {
            Path source=output.resolve("PairingFailureHarness.java");
            Files.write(source,(HARNESS_PREFIX+pair+response+discard+"}\n"+store+profile+endpointPolicy+"\nclass BuildConfig { static final boolean DEBUG=false; }\n"+STORE_STUBS).getBytes(StandardCharsets.UTF_8));
            Path log=output.resolve("android/util/Log.java");Files.createDirectories(log.getParent());
            Files.write(log,"package android.util; public class Log { public static int e(String tag,String message,Throwable error){return 0;} }".getBytes(StandardCharsets.UTF_8));
            Object compiler=Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
            assertNotNull(compiler);
            String cp=Paths.get(JSONObject.class.getProtectionDomain().getCodeSource().getLocation().toURI()).toString();
            try(java.io.ByteArrayOutputStream diagnostics=new java.io.ByteArrayOutputStream()) {
                String[] args={"-classpath",cp,"-d",output.toString(),"-proc:none",source.toString(),log.toString()};
                int status=(Integer)Class.forName("javax.tools.Tool").getMethod("run",java.io.InputStream.class,java.io.OutputStream.class,java.io.OutputStream.class,String[].class).invoke(compiler,null,diagnostics,diagnostics,args);
                assertEquals("actual production callback/store must compile: "+diagnostics.toString("UTF-8"),0,status);
            }
            loader=new URLClassLoader(new java.net.URL[]{output.toUri().toURL()},RelayPairingFailureTest.class.getClassLoader()) {
                @Override protected synchronized Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
                    // Only this declared Android logging boundary is mocked. Gradle's
                    // android.jar stub otherwise wins parent-first loading and throws.
                    if (!"android.util.Log".equals(name)) return super.loadClass(name, resolve);
                    Class<?> type=findLoadedClass(name);
                    if(type==null) type=findClass(name);
                    if(resolve) resolveClass(type);
                    return type;
                }
            };
            harness=Class.forName("PairingFailureHarness",true,loader);complete=true;
        } finally {if(!complete)cleanup();}
    }
    private static String between(String source,String first,String last){int a=source.indexOf(first),b=source.indexOf(last,a);assertTrue(first,a>=0&&b>a);return source.substring(a,b);}
    private static Path findSource(){
        for(Path root=Paths.get(System.getProperty("user.dir")).toAbsolutePath();root!=null;root=root.getParent())
            for(String relative:new String[]{"src/main/java/org/freevia/qcremote","app/src/main/java/org/freevia/qcremote","apps/android/android/app/src/main/java/org/freevia/qcremote"})
                if(Files.isRegularFile(root.resolve(relative).resolve("QcRelayPlugin.java")))return root.resolve(relative);
        throw new AssertionError("actual production relay source unavailable");
    }
    private Object begin(boolean fail) throws Exception {Object value=harness.getConstructor().newInstance();harness.getMethod("begin",boolean.class).invoke(value,fail);return value;}
    private JSONObject report(Object value)throws Exception{return new JSONObject((String)harness.getMethod("report").invoke(value));}
    @Test public void lostResponseRetainsOldCredentialAndConsumesTheSinglePost() throws Exception {
        Object value=begin(false);harness.getMethod("loss").invoke(value);
        JSONObject facts=report(value);assertEquals(1,facts.getInt("posts"));assertEquals(0,facts.getInt("starts"));assertTrue(facts.getBoolean("oldDurable"));assertTrue(facts.getBoolean("browserConsumed"));assertEquals("PAIRING_NETWORK_ERROR",facts.getString("error"));
    }
    @Test public void serverReceiptThenFailedSaveNeverStartsNewConnection() throws Exception {
        Object value=begin(true);harness.getMethod("response",boolean.class).invoke(value,false);
        JSONObject facts=report(value);assertEquals(1,facts.getInt("posts"));assertEquals(0,facts.getInt("starts"));assertTrue(facts.getBoolean("oldDurable"));assertEquals("fixture-old",facts.getString("previousId"));assertEquals("PAIRING_STORAGE_ERROR",facts.getString("error"));assertTrue(facts.getBoolean("browserConsumed"));
    }
    @Test public void durableSavePrecedesConnectionAndFallbackWaitsForCurrentIdentityAck() throws Exception {
        Object value=begin(false);harness.getMethod("response",boolean.class).invoke(value,false);
        assertEquals(1,report(value).getInt("starts"));assertEquals("fixture-old",report(value).getString("previousId"));
        assertEquals(false,harness.getMethod("ack",String.class,boolean.class).invoke(value,"fixture-old",true));
        assertEquals(false,harness.getMethod("ack",String.class,boolean.class).invoke(value,"wrong-device",false));
        assertEquals("fixture-old",report(value).getString("previousId"));
        assertEquals(true,harness.getMethod("ack",String.class,boolean.class).invoke(value,"fixture-old",false));
        assertEquals("",report(value).getString("previousId"));
    }
    @Test public void differentOwnerReceiptIsWithdrawnWithoutReplacingHeldCredential() throws Exception {
        Object value=begin(false);harness.getMethod("response",boolean.class).invoke(value,true);
        JSONObject facts=report(value);assertEquals(0,facts.getInt("starts"));assertEquals(1,facts.getInt("withdrawals"));assertTrue(facts.getBoolean("oldDurable"));assertEquals("PAIRING_OWNER_CHANGED",facts.getString("error"));
    }
    @Test public void localUnpairRetainsOnlyEncryptedWithdrawalUntilAnOnlineRetrySucceeds() throws Exception {
        Object value=harness.getConstructor().newInstance();
        JSONObject facts=new JSONObject((String)harness.getMethod("unpairAndRetry").invoke(value));
        assertFalse(facts.getBoolean("pairedAfterClear"));
        assertTrue(facts.getBoolean("encryptedOnly"));
        assertTrue(facts.getBoolean("pairBlocked"));
        assertTrue(facts.getBoolean("retrySameCredential"));
        assertTrue(facts.getBoolean("legacyMigrated"));
        assertTrue(facts.getBoolean("staleCannotClear"));
        assertTrue(facts.getBoolean("confirmed"));
        assertFalse(facts.getBoolean("pendingAfterConfirmation"));
        assertTrue(facts.getBoolean("newPairAllowed"));
    }
    @Test public void unpairBeforeReplacementAckRetainsBothCredentialsAndRetiresOnlyConfirmedOnes() throws Exception {
        Object value=harness.getConstructor().newInstance();
        JSONObject facts=new JSONObject((String)harness.getMethod("unpairDuringReplacement").invoke(value));
        assertFalse(facts.getBoolean("paired"));
        assertEquals("fixture-new",facts.getString("first"));
        assertEquals("fixture-old",facts.getString("remaining"));
        assertTrue(facts.getBoolean("remainingCredentialIntact"));
        assertTrue(facts.getBoolean("staleCannotDropRemaining"));
        assertTrue(facts.getBoolean("pairBlocked"));
        assertTrue(facts.getBoolean("bothConfirmed"));
        assertFalse(facts.getBoolean("pending"));
    }
    @Test public void secondReplacementIsRefusedBeforePostingAndCannotOverwriteFirstPredecessor() throws Exception {
        Object value=begin(false);harness.getMethod("response",boolean.class).invoke(value,false);
        harness.getMethod("begin",boolean.class).invoke(value,false);
        JSONObject facts=report(value);
        assertEquals(1,facts.getInt("posts"));
        assertEquals("PAIRING_REPLACEMENT_PENDING",facts.getString("error"));
        assertEquals("fixture-old",facts.getString("previousId"));
        assertTrue((Boolean)harness.getMethod("directOverwriteRefused").invoke(value));
        assertEquals("fixture-old",report(value).getString("previousId"));
        assertEquals(true,harness.getMethod("ack",String.class,boolean.class).invoke(value,"fixture-old",false));
        harness.getMethod("begin",boolean.class).invoke(value,false);
        assertEquals(2,report(value).getInt("posts"));
    }
    @Test public void forbiddenLegacyWithdrawalCanBeLocallyDiscardedWithoutContactAndThenPairAgain() throws Exception {
        Object value=harness.getConstructor().newInstance();
        JSONObject facts=new JSONObject((String)harness.getMethod("discardForbiddenWithdrawal").invoke(value));
        assertTrue(facts.getString("warning").contains("Remote revocation could not be confirmed"));
        assertFalse(facts.getBoolean("pending"));
        assertTrue(facts.getBoolean("paired"));
        assertEquals(0,facts.getInt("posts"));
        assertEquals(0,facts.getInt("withdrawals"));
        assertTrue(facts.getBoolean("permittedRetained"));
    }
    @AfterClass public static void cleanup() throws Exception {
        if(loader!=null){loader.close();loader=null;}
        if(output!=null&&Files.exists(output))try(java.util.stream.Stream<Path> paths=Files.walk(output)){for(Path p:(Iterable<Path>)paths.sorted(Comparator.reverseOrder())::iterator)Files.delete(p);}
        output=null;
    }
    private static final String HARNESS_PREFIX=String.join("\n",
        "",
        "import org.json.JSONObject;",
        "import java.nio.charset.StandardCharsets;",
        "import java.io.IOException;",
        "import java.util.*;",
        "import java.net.URI;",
        "public class PairingFailureHarness {",
        "    final Context context=new Context(); final Http http=new Http();",
        "    Object browserPairing=new Object(); boolean browserPairingCompleting=true;",
        "    String relayState=\"stopped\"; int starts,withdrawals; final PluginCall result=new PluginCall();",
        "    public PairingFailureHarness() throws Exception {",
        "        new RelayCredentialStore(context).save(\"https://qcrc.freevia.org\",\"o\".repeat(32),\"fixture-old\",\"owner@example.test\");",
        "    }",
        "    Context getContext(){return context;}",
        "    static Http pairingHttpClient(Http base){return base;}",
        "    // The HTTP request builder has its own actual-request capture tests. This seam captures enqueue only.",
        "    static Request pairingRequest(String endpoint,String code,String name,RelayCredentialStore.Snapshot held){return new Request();}",
        "    void startService(){",
        "        if (!\"fixture-new\".equals(context.prefs.disk.get(\"device_id\"))) throw new AssertionError(\"connect before durable save\");",
        "        starts++;",
        "    }",
        "    void withdrawCredential(String endpoint,String deviceId,String credential){withdrawals++;}",
        "    public void begin(boolean saveFailure){context.prefs.failNextCommit=saveFailure;pairWithCode(\"https://qcrc.freevia.org\",\"fixture-code\",\"fixture\",result,true);}",
        "    public void response(boolean differentOwner) throws Exception {",
        "        JSONObject receipt=new JSONObject().put(\"deviceId\",\"fixture-new\").put(\"deviceCredential\",\"n\".repeat(32))",
        "            .put(\"previousDeviceReplacementPending\",true).put(\"identityProvider\",\"google\")",
        "            .put(\"pairing\",new JSONObject().put(\"email\",differentOwner?\"other@example.test\":\"owner@example.test\"));",
        "        http.pending.onResponse(new Call(http),new Response(receipt.toString()));",
        "    }",
        "    public void loss() throws Exception {http.pending.onFailure(new Call(http),new IOException(\"synthetic response lost\"));}",
        "    public boolean directOverwriteRefused() throws Exception {",
        "        try {new RelayCredentialStore(context).save(\"https://qcrc.freevia.org\",\"t\".repeat(32),\"fixture-third\",\"owner@example.test\");return false;}",
        "        catch(IllegalStateException expected){return true;}",
        "    }",
        "    public String discardForbiddenWithdrawal() throws Exception {",
        "        RelayCredentialStore store=new RelayCredentialStore(context);",
        "        context.prefs.edit().putString(\"endpoint\",\"https://legacy.example\").commit();",
        "        RelayCredentialStore.Snapshot old=store.beginUnpair(store.snapshot());",
        "        String warning=discardForbiddenPendingRevocation(store,old);",
        "        boolean pending=store.hasPendingRevocation();",
        "        store.save(\"https://qcrc.freevia.org\",\"n\".repeat(32),\"fixture-new\",\"owner@example.test\");",
        "        boolean paired=store.paired();RelayCredentialStore.Snapshot allowed=store.beginUnpair(store.snapshot());",
        "        boolean permittedRetained=!store.discardForbiddenRevocation(allowed,false) && store.hasPendingRevocation();",
        "        return new JSONObject().put(\"warning\",warning).put(\"pending\",pending).put(\"paired\",paired)",
        "            .put(\"posts\",http.posts).put(\"withdrawals\",withdrawals).put(\"permittedRetained\",permittedRetained).toString();",
        "    }",
        "    public String report() throws Exception {",
        "        return new JSONObject().put(\"posts\",http.posts).put(\"starts\",starts).put(\"withdrawals\",withdrawals)",
        "            .put(\"oldDurable\",\"fixture-old\".equals(context.prefs.disk.get(\"device_id\")))",
        "            .put(\"browserConsumed\",browserPairing==null).put(\"error\",result.error==null?\"\":result.error)",
        "            .put(\"previousId\",new RelayCredentialStore(context).previousDeviceId()==null?\"\":new RelayCredentialStore(context).previousDeviceId()).toString();",
        "    }",
        "    public boolean ack(String previous,boolean stale) throws Exception {",
        "        RelayCredentialStore store=new RelayCredentialStore(context); RelayCredentialStore.Snapshot current=store.snapshot();",
        "        if(stale)current=new RelayCredentialStore.Snapshot(current.endpoint,current.credential,current.deviceId,current.owner,\"stale\",false);",
        "        return store.acknowledgeReplacement(current,previous);",
        "    }",
        "    public String unpairAndRetry() throws Exception {",
        "        RelayCredentialStore store=new RelayCredentialStore(context);",
        "        RelayCredentialStore.Snapshot pending=store.beginUnpair(store.snapshot());",
        "        boolean paired=store.paired();",
        "        String record=(String)context.prefs.disk.get(\"pending_revocation_encrypted\");",
        "        boolean encryptedOnly=!record.contains(\"o\".repeat(32)) && !context.prefs.disk.containsKey(\"credential\");",
        "        boolean blocked=false;try{store.save(\"https://qcrc.freevia.org\",\"n\".repeat(32),\"fixture-new\",\"owner@example.test\");}catch(IllegalStateException expected){blocked=true;}",
        "        context.prefs.edit().putString(\"pending_revocation_encrypted\",new org.json.JSONArray(record).getJSONObject(0).toString()).commit();",
        "        RelayCredentialStore.Snapshot retry=store.beginUnpair(store.snapshot());",
        "        boolean migrated=((String)context.prefs.disk.get(\"pending_revocation_encrypted\")).startsWith(\"[\");",
        "        boolean same=RelayCredentialStore.sameIdentity(pending,retry);",
        "        RelayCredentialStore.Snapshot stale=new RelayCredentialStore.Snapshot(pending.endpoint,pending.credential,pending.deviceId,\"\",\"stale\",false);",
        "        boolean staleCannotClear=!store.completeRevocation(stale) && store.hasPendingRevocation();",
        "        boolean confirmed=store.completeRevocation(retry);boolean remaining=store.hasPendingRevocation();",
        "        store.save(\"https://qcrc.freevia.org\",\"n\".repeat(32),\"fixture-new\",\"owner@example.test\");",
        "        return new JSONObject().put(\"pairedAfterClear\",paired).put(\"encryptedOnly\",encryptedOnly).put(\"pairBlocked\",blocked)",
        "            .put(\"retrySameCredential\",same).put(\"legacyMigrated\",migrated).put(\"staleCannotClear\",staleCannotClear).put(\"confirmed\",confirmed)",
        "            .put(\"pendingAfterConfirmation\",remaining).put(\"newPairAllowed\",store.paired()).toString();",
        "    }",
        "    public String unpairDuringReplacement() throws Exception {",
        "        RelayCredentialStore store=new RelayCredentialStore(context);",
        "        store.save(\"https://qcrc.freevia.org\",\"n\".repeat(32),\"fixture-new\",\"owner@example.test\");",
        "        RelayCredentialStore.Snapshot first=store.beginUnpair(store.snapshot());boolean paired=store.paired();",
        "        boolean confirmedFirst=store.completeRevocation(first);",
        "        RelayCredentialStore.Snapshot remaining=store.beginUnpair(store.snapshot());",
        "        boolean stale=!store.completeRevocation(first) && store.hasPendingRevocation();",
        "        boolean blocked=false;try{store.save(\"https://qcrc.freevia.org\",\"n\".repeat(32),\"another\",\"owner@example.test\");}catch(IllegalStateException expected){blocked=true;}",
        "        boolean both=confirmedFirst && store.completeRevocation(remaining);",
        "        return new JSONObject().put(\"paired\",paired).put(\"first\",first.deviceId).put(\"remaining\",remaining.deviceId)",
        "            .put(\"remainingCredentialIntact\",\"o\".repeat(32).equals(remaining.credential)).put(\"staleCannotDropRemaining\",stale)",
        "            .put(\"pairBlocked\",blocked).put(\"bothConfirmed\",both).put(\"pending\",store.hasPendingRevocation()).toString();",
        "    }",
        "    static class PluginCall {",
        "        String error; void resolve(JSObject object){}",
        "        void reject(String message,String code){error=code;}",
        "        void reject(String message,String code,Exception exception){error=code;}",
        "    }",
        "    static class Http {int posts; Callback pending; Call newCall(Request request){posts++;return new Call(this);}}",
        "    static class Call {final Http owner; Call(Http owner){this.owner=owner;} void enqueue(Callback callback){owner.pending=callback;}}",
        "    interface Callback {void onFailure(Call call,IOException error);void onResponse(Call call,Response response);}",
        "    static class Request {}",
        "    static class Response implements AutoCloseable {",
        "        final String json;Response(String json){this.json=json;}boolean isSuccessful(){return true;}int code(){return 200;}",
        "        Response body(){return this;}String string(){return json;}public void close(){}",
        "    }",
        "    static class JSObject extends JSONObject {}",
        "    static class QcUsbPlugin {static boolean relayForegroundServiceEligible(){return true;} static boolean relayDemoAvailable(){return false;}}")+"\n";
    private static final String STORE_STUBS=String.join("\n",
        "",
        "class Context {",
        "    static final int MODE_PRIVATE=0;final SharedPreferences prefs=new SharedPreferences();",
        "    Context getApplicationContext(){return this;} SharedPreferences getSharedPreferences(String ignored,int mode){return prefs;}",
        "}",
        "/** Native storage boundary: a failed commit mutates memory but not durable bytes, matching Android semantics. */",
        "class SharedPreferences {",
        "    final Map<String,Object> memory=new HashMap<>(),disk=new HashMap<>();boolean failNextCommit;",
        "    String getString(String key,String fallback){Object value=memory.get(key);return value==null?fallback:(String)value;}",
        "    boolean getBoolean(String key,boolean fallback){Object value=memory.get(key);return value==null?fallback:(Boolean)value;}",
        "    boolean contains(String key){return memory.containsKey(key);}Editor edit(){return new Editor();}",
        "    class Editor {",
        "        final Map<String,Object> pending=new HashMap<>(memory);",
        "        Editor putString(String key,String value){if(value==null)pending.remove(key);else pending.put(key,value);return this;}",
        "        Editor putBoolean(String key,boolean value){pending.put(key,value);return this;}",
        "        Editor remove(String key){pending.remove(key);return this;}Editor clear(){pending.clear();return this;}",
        "        boolean commit(){memory.clear();memory.putAll(pending);if(failNextCommit){failNextCommit=false;return false;}disk.clear();disk.putAll(memory);return true;}",
        "        void apply(){commit();}",
        "    }",
        "}",
        "/** Crypto is explicitly mocked; this test proves ordering/retention, not Android Keystore encryption. */",
        "class Cipher {",
        "    static final int ENCRYPT_MODE=1,DECRYPT_MODE=2;",
        "    static Cipher getInstance(String name){return new Cipher();}void init(int mode,Object key){}void init(int mode,Object key,Object params){}",
        "    byte[] doFinal(byte[] bytes){return bytes;}byte[] getIV(){return new byte[12];}",
        "}",
        "class Base64 {static final int NO_WRAP=0;static String encodeToString(byte[] value,int flags){return java.util.Base64.getEncoder().encodeToString(value);}static byte[] decode(String value,int flags){return java.util.Base64.getDecoder().decode(value);}}",
        "class GCMParameterSpec {GCMParameterSpec(int bits,byte[] iv){}}",
        "class RelayState {static RelayState parse(String state){return new RelayState();}String wireValue(){return \"stopped\";}}")+"\n";
}
