package org.freevia.qcremote;

import android.test.ActivityInstrumentationTestCase2;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;
import org.json.JSONObject;

/** Foreground emulator proof: real native bridge and the UI's existing demo store, no relay network or USB. */
@SuppressWarnings("deprecation")
public final class QcDemoMcpInstrumentationTest extends ActivityInstrumentationTestCase2<MainActivity> {
    public QcDemoMcpInstrumentationTest() { super(MainActivity.class); }

    private static JSONObject invoke(String method, JSONObject params, QcBrokerTarget target) throws Exception {
        CompletableFuture<JSONObject> result = QcUsbPlugin.invokeFromRelay(method, params, new JSONObject(), target);
        try { return result.get(15, TimeUnit.SECONDS); }
        finally { if (!result.isDone()) QcUsbPlugin.expireRelayInvocation(result); }
    }

    private int uiVolume(MainActivity activity) throws Exception {
        AtomicReference<String> answer = new AtomicReference<>();
        CountDownLatch ready = new CountDownLatch(1);
        getInstrumentation().runOnMainSync(() -> activity.getBridge().getWebView().evaluateJavascript(
            "(()=>{const knob=document.querySelector('[aria-label=\"Master volume knob\"]');return knob?Number(knob.getAttribute('aria-valuenow')):null})()",
            value -> { answer.set(value); ready.countDown(); }));
        assertTrue("The WebView must answer its foreground UI read", ready.await(5, TimeUnit.SECONDS));
        return answer.get() == null || "null".equals(answer.get()) ? -1 : Integer.parseInt(answer.get());
    }

    private void waitUiVolume(MainActivity activity, int expected) throws Exception {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
        while (System.nanoTime() < deadline) {
            if (uiVolume(activity) == expected) return;
            Thread.sleep(50);
        }
        assertEquals("The same visible UI store must reflect the native MCP mutation", expected, uiVolume(activity));
    }

    public void testNativeMcpUsesTheVisibleDemoAndRestoresItsValue() throws Exception {
        assertFalse("Use an unpaired emulator for this local-only proof",
            new RelayCredentialStore(getInstrumentation().getTargetContext()).paired());
        MainActivity activity = getActivity();
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(30);
        while (!QcUsbPlugin.relayDemoAvailable() && System.nanoTime() < deadline) Thread.sleep(50);
        assertFalse("This proof must never use a physical QC", QcUsbPlugin.relaySessionAvailable());
        assertTrue("The foreground UI must register its actual demo provider", QcUsbPlugin.relayDemoAvailable());
        QcBrokerTarget target = QcUsbPlugin.relayBrokerTarget();
        assertNotNull(target);
        assertEquals("demo", target.source);

        JSONObject status = invoke("system.status", new JSONObject(), target);
        assertEquals(GeneratedGatewayMethods.API_VERSION, status.getInt("gatewayApiVersion"));
        assertEquals("android-demo-relay", status.getString("transport"));
        assertTrue(target.same(QcBrokerTarget.parse(status.get("brokerTarget"))));
        int original = invoke("device.masterVolume", new JSONObject(), target).getInt("value");
        int changed = original == 0 ? 1 : original - 1;
        waitUiVolume(activity, original);
        try {
            invoke("device.setMasterVolume", new JSONObject().put("value", changed).put("expectedValue", original), target);
            assertEquals(changed, invoke("device.masterVolume", new JSONObject(), target).getInt("value"));
            waitUiVolume(activity, changed);
            assertFalse(QcUsbPlugin.relaySessionAvailable());
        } finally {
            // Never replay cleanup onto another target or overwrite a later human edit.
            if (target.same(QcUsbPlugin.relayBrokerTarget()) && !QcUsbPlugin.relaySessionAvailable()) {
                int current = invoke("device.masterVolume", new JSONObject(), target).getInt("value");
                if (current == changed) {
                    invoke("device.setMasterVolume", new JSONObject().put("value", original).put("expectedValue", changed), target);
                    waitUiVolume(activity, original);
                }
            }
        }
    }
}
