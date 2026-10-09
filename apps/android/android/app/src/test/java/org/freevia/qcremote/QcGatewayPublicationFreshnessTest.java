package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.nio.file.*;
import java.nio.charset.StandardCharsets;
import java.net.URLClassLoader;
import org.json.JSONObject;
import org.junit.*;

/** Executes the actual publication, readback and no-op branches; native policy is an explicit boundary fake. */
public class QcGatewayPublicationFreshnessTest {
    private static Path output;
    private static URLClassLoader loader;
    private static Class<?> harness;
    private static String read(Path p) throws Exception { return new String(Files.readAllBytes(p), StandardCharsets.UTF_8); }
    private static String between(String text,String a,String b) {
        int start=text.indexOf(a), end=text.indexOf(b,start);
        assertTrue("production boundary "+a,start>=0&&end>start); return text.substring(start,end);
    }
    private static Path sourceDirectory() {
        for(Path p=Paths.get(System.getProperty("user.dir")).toAbsolutePath();p!=null;p=p.getParent()) {
            for(String relative:new String[]{"src/main/java/org/freevia/qcremote","app/src/main/java/org/freevia/qcremote","apps/android/android/app/src/main/java/org/freevia/qcremote"}) {
                Path candidate=p.resolve(relative);if(Files.isRegularFile(candidate.resolve("QcUsbPlugin.java")))return candidate;
            }
        }
        throw new AssertionError("Actual Android source not found");
    }
    @BeforeClass public static void compile() throws Exception {
        Path directory=sourceDirectory();String plugin=read(directory.resolve("QcUsbPlugin.java"));
        String publication=between(plugin,"    private void publishStateBatch(","    private void recoverSessionAfterBackup(");
        String readback=between(plugin,"    private CompletableFuture<org.json.JSONObject> relayGatewayWriteReadback(","    private CompletableFuture<org.json.JSONObject> executeRelayPlan(");
        String noop=between(plugin,"                if (plan.noOp) {","                org.json.JSONObject verification = new org.json.JSONObject(plan.verificationJson);");
        String pending=between(read(directory.resolve("QcUsbPendingState.java")),"final class PendingGatewayTransaction {","final class PendingCatalogRead {")
            .replace("com.getcapacitor.JSObject","JSObject"); // only platform boundary type
        String fence=read(directory.resolve("QcObservationFence.java")).replace("package org.freevia.qcremote;", "");
        String echo=read(directory.resolve("QcSettingsEchoEvidence.java")).replace("package org.freevia.qcremote;", "");
        output=Files.createTempDirectory("qc-gateway-evidence-");boolean ready=false;
        try {
            Path java=output.resolve("QcUsbPlugin.java");
            Files.write(java,(PREFIX+publication+readback+"void noOp() throws Exception {\n"+noop+"submissions++; }\n}\n"+STUBS+pending+fence+echo).getBytes(StandardCharsets.UTF_8));
            Object compiler=Class.forName("javax.tools.ToolProvider").getMethod("getSystemJavaCompiler").invoke(null);
            String cp=Paths.get(JSONObject.class.getProtectionDomain().getCodeSource().getLocation().toURI()).toString();
            try(java.io.ByteArrayOutputStream diagnostics=new java.io.ByteArrayOutputStream()) {
                String[] args={"-classpath",cp,"-d",output.toString(),"-proc:none",java.toString()};
                int result=(Integer)Class.forName("javax.tools.Tool").getMethod("run",java.io.InputStream.class,java.io.OutputStream.class,java.io.OutputStream.class,String[].class).invoke(compiler,null,diagnostics,diagnostics,args);
                assertEquals(diagnostics.toString("UTF-8"),0,result);
            }
            loader=new URLClassLoader(new java.net.URL[]{output.toUri().toURL()},QcGatewayPublicationFreshnessTest.class.getClassLoader());
            harness=Class.forName("QcUsbPlugin",true,loader);ready=true;
        } finally {if(!ready)cleanup();}
    }
    private JSONObject run(String test) throws Exception { return new JSONObject((String)harness.getMethod(test).invoke(harness.getConstructor().newInstance())); }
    @Test public void acceptedBeforeWriteButPublishedAfterwardsDoesNotVerify() throws Exception {
        JSONObject r=run("delayedPublication");assertEquals(0,r.getLong("afterDelayed"));assertEquals(11,r.getLong("afterFresh"));
    }
    @Test public void tempoAndNewEpochPublicationCannotAdvanceOldWrite() throws Exception {
        JSONObject r=run("unrelatedPublication");assertEquals(0,r.getLong("afterTempo"));assertEquals(0,r.getLong("afterReset"));
    }
    @Test public void retainedAndUnrelatedRawEchoCannotVerifyCachedSameValue() throws Exception {
        JSONObject r=run("rawReadbacks");assertFalse(r.getJSONObject("stale").has("observation"));assertFalse(r.getJSONObject("unrelated").has("observation"));assertFalse(r.getJSONObject("oldEpoch").has("observation")); JSONObject fresh=r.getJSONObject("fresh");assertFalse(fresh.getBoolean("verified"));assertEquals("accepted_unverified",fresh.getString("verification"));assertEquals("matching_publication_after_native_admission",fresh.getJSONObject("observation").getString("kind"));assertFalse(fresh.getJSONObject("observation").getBoolean("causallyCorrelated"));assertFalse(fresh.has("changed"));assertFalse(fresh.has("readback"));
    }
    @Test public void noOpReturnsUnchangedWithoutSubmissionButRefusesExpiredAdmission() throws Exception {
        JSONObject r=run("noOpResults");assertFalse(r.getBoolean("changed"));assertFalse(r.getBoolean("requestSent"));assertEquals(0,r.getInt("submissions"));assertTrue(r.getBoolean("expiredRefused"));
    }
    @AfterClass public static void cleanup() throws Exception {
        try {if(loader!=null)loader.close();} finally {
            loader=null;if(output!=null){try(java.util.stream.Stream<Path> paths=Files.walk(output)){for(Path p:(Iterable<Path>)paths.sorted(java.util.Comparator.reverseOrder())::iterator)Files.delete(p);}assertFalse(Files.exists(output));output=null;}
        }
    }
    private static final String PREFIX = """
import org.json.*; import java.util.*; import java.util.concurrent.*; import java.util.concurrent.atomic.AtomicBoolean;
public class QcUsbPlugin {
 final Object stateEventLock=new Object(); final Decoder stateDecoder=new Decoder();
 final QcSettingsEchoEvidence settingsEchoEvidence=new QcSettingsEchoEvidence();
 final Operations pendingOperations=new Operations(); final Journal stateEventLog=new Journal();
 final QcNativeStateDecoder.PlannedGatewayWrite plan=new QcNativeStateDecoder.PlannedGatewayWrite();
 final CompletableFuture<JSONObject> result=new CompletableFuture<>(); final String admission="fixture";
 long nextStateSequence=1,lastStateAt,currentMasterVolumeObservedAt; int currentMasterVolume,currentPosition,submissions; String currentSetlist="",stateStreamId="fixture"; JSObject latestTempoClock;
 final ScheduledExecutorService keepalive=Executors.newSingleThreadScheduledExecutor();
 static long monotonicMillis(){return 0;} static int publishedSettingsType(String method){return 9;}
 static <T> CompletableFuture<T> failedFuture(Throwable e){CompletableFuture<T> f=new CompletableFuture<>();f.completeExceptionally(e);return f;}
 void notifyListeners(String name,JSObject frame,boolean retained){} void resolvePendingReady(){} void resolvePendingGatewayTransactions(long sequence,long now){}
 static String ticket(long sequence,long revision){return "{\\"connection\\":1,\\"epoch\\":{\\"sessionId\\":\\"fixture\\",\\"revision\\":"+revision+"},\\"sequence\\":"+sequence+"}";}
 PendingGatewayTransaction operation(){PendingGatewayTransaction p=new PendingGatewayTransaction(plan,0,0);p.admission="fixture";pendingOperations.entry=new QcPendingOperations.Entry<>(p);return p;}
 void publish(long sequence,long revision,String kind){publishStateBatch(new QcNativeStateDecoder.StatePublication(ticket(sequence,revision),kind));}
 public String delayedPublication(){try{PendingGatewayTransaction p=operation();publish(10,0,"scene");long before=p.observationSequence();publish(11,0,"scene");return new JSONObject().put("afterDelayed",before).put("afterFresh",p.observationSequence()).toString();}finally{keepalive.shutdownNow();}}
 public String unrelatedPublication(){try{PendingGatewayTransaction p=operation();publish(11,0,"");long before=p.observationSequence();stateDecoder.revision=1;publish(1,1,"scene");return new JSONObject().put("afterTempo",before).put("afterReset",p.observationSequence()).toString();}finally{keepalive.shutdownNow();}}
 JSONObject readback(long sequence,long revision,byte value) throws Exception {QcSettingsEchoEvidence.Fence f=new QcSettingsEchoEvidence.Fence();f.operation="fixture";settingsEchoEvidence.observe(9,ticket(sequence,revision),new byte[]{value});return relayGatewayWriteReadback("fixture",new JSONObject(),"fixture","scope",f,0,new JSONObject().put("verified",false)).get(1,TimeUnit.SECONDS);}
 public String rawReadbacks() throws Exception {try{return new JSONObject().put("stale",readback(10,0,(byte)1)).put("unrelated",readback(11,0,(byte)0)).put("fresh",readback(12,0,(byte)1)).put("oldEpoch",readback(13,1,(byte)1)).toString();}finally{keepalive.shutdownNow();}}
 public String noOpResults() throws Exception {try{plan.noOp=true;noOp();JSONObject value=result.get();stateDecoder.expired=true;boolean refused=false;try{noOp();}catch(Exception expected){refused=true;}return value.put("submissions",submissions).put("expiredRefused",refused).toString();}finally{keepalive.shutdownNow();}}
""";
    private static final String STUBS = """
class JSObject extends JSONObject { public JSObject put(String k,Object v){super.put(k,v);return this;} public JSObject put(String k,int v){super.put(k,v);return this;} public JSObject put(String k,long v){super.put(k,v);return this;} public JSObject put(String k,boolean v){super.put(k,v);return this;} public JSObject put(String k,double v){super.put(k,v);return this;} String getString(String k,String fallback){return optString(k,fallback);} int getInteger(String k){return getInt(k);} int getInteger(String k,int fallback){return optInt(k,fallback);} }
class JSArray extends JSONArray {}
class Journal {boolean add(JSObject frame){return true;}}
class QcNativeStateDecoder {
 static class PlannedGatewayWrite {Set<String> observedStateKinds=Set.of("scene");boolean noOp;}
 static class StatePublication {String observationTicket;QcObservationFence observationFence;long observedAt;List<JSObject> states=new ArrayList<>();JSObject tempoClock;
 StatePublication(String ticket,String kind){observationTicket=ticket;observationFence=new QcObservationFence(ticket);if(kind.isEmpty())tempoClock=new JSObject().put("currentBeat",0).put("currentBar",0).put("currentTick",0).put("countInBeatsRemaining",0).put("countInBarsRemaining",0);else states.add(new JSObject().put("kind",kind));}}
}
class Decoder {
 long revision;boolean expired;
 boolean operationObservationIsFresh(String operation,String ticket){QcObservationFence f=new QcObservationFence(ticket);return "fixture".equals(operation)&&f.revision==0&&f.sequence>10;}
 boolean observationCurrent(String ticket){return new QcObservationFence(ticket).revision==revision;}
 long gatewayReadbackRetryDelay(String method,int attempt){return attempt<2?0:-1;}
 long gatewayReadScopeRemaining(String scope,long now){if(expired)throw new IllegalStateException("expired");return 100;}
 long operationRemaining(String scope,long now){return gatewayReadScopeRemaining(scope,now);}
 JSONObject publishedSettings(int type){return new JSONObject().put("cachedSameValue",true);}
 boolean gatewayWriteEchoMatches(String method,JSONObject params,int type,byte[] payload){return payload.length==1&&payload[0]==1;}
 boolean gatewayReadbackMatches(String method,JSONObject params,JSONObject response){return true;}
}
class QcPendingOperations {static class Entry<T>{T operation;Entry(T value){operation=value;}}}
class Operations {QcPendingOperations.Entry<PendingGatewayTransaction> entry;List<QcPendingOperations.Entry<PendingGatewayTransaction>> entries(Class<PendingGatewayTransaction> type){return List.of(entry);}}
""";
}
