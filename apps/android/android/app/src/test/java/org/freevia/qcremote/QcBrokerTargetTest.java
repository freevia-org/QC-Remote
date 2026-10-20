package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.json.JSONObject;
import org.junit.Test;

public class QcBrokerTargetTest {
    @Test public void queuedWorkCannotAdmitAgainstAReplacementTarget() {
        QcBrokerTarget.Registry registry = new QcBrokerTarget.Registry();
        QcBrokerTarget admitted = registry.select("physical", "usb", 1);
        int[] effects = { 0 };
        Runnable queued = () -> {
            admitted.assertCurrent(registry.select("physical", "usb", 2));
            effects[0]++;
        };
        try { queued.run(); fail("A replacement target must reject queued work"); }
        catch (RelayException refused) { assertEquals("TARGET_SUPERSEDED", refused.code); }
        assertEquals(0, effects[0]);
    }
    @Test public void aStableProviderKeepsItsBrokerIssuedTarget() {
        QcBrokerTarget.Registry registry = new QcBrokerTarget.Registry();
        QcBrokerTarget first = registry.select("demo", "webview-registration", 5);
        assertSame(first, registry.select("demo", "webview-registration", 5));
        assertNotEquals("webview-registration", first.sessionId);
        assertTrue(first.same(QcBrokerTarget.parse(first.json())));
    }

    @Test public void returningToTheSameDemoStoreNeverResurrectsItsOldIdentity() {
        QcBrokerTarget.Registry registry = new QcBrokerTarget.Registry();
        QcBrokerTarget demo = registry.select("demo", "same-store", 1);
        QcBrokerTarget physical = registry.select("physical", "usb", 3);
        QcBrokerTarget returned = registry.select("demo", "same-store", 1);
        assertFalse(demo.same(physical));
        assertFalse(demo.same(returned));
        assertTrue(returned.generation > physical.generation);
        registry.clear();
        assertFalse(returned.same(registry.select("demo", "same-store", 1)));
    }

    @Test public void usbReconnectAndUiReregistrationEachInvalidateTheirOldTarget() {
        QcBrokerTarget.Registry registry = new QcBrokerTarget.Registry();
        QcBrokerTarget usb = registry.select("physical", "usb", 1);
        assertFalse(usb.same(registry.select("physical", "usb", 2)));
        QcBrokerTarget demo = registry.select("demo", "ui", 1);
        assertFalse(demo.same(registry.select("demo", "ui", 2)));
        assertNull(registry.select(null, null, 0));
    }

    @Test public void targetValidationDoesNotCoerceMalformedOrUnknownFields() throws Exception {
        for (String raw : new String[]{"null", "{}", "{\"source\":\"virtual\",\"sessionId\":\"x\",\"generation\":1}",
                "{\"source\":\"demo\",\"sessionId\":\"\",\"generation\":1}",
                "{\"source\":\"demo\",\"sessionId\":\"   \",\"generation\":1}",
                "{\"source\":\"demo\",\"sessionId\":\"x\",\"generation\":0}",
                "{\"source\":\"demo\",\"sessionId\":\"x\",\"generation\":1.5}",
                "{\"source\":\"demo\",\"sessionId\":\"x\",\"generation\":\"1\"}",
                "{\"source\":\"demo\",\"sessionId\":\"x\",\"generation\":1,\"extra\":true}"}) {
            try { QcBrokerTarget.parse("null".equals(raw) ? JSONObject.NULL : new JSONObject(raw)); fail(raw); }
            catch (IllegalArgumentException expected) { /* Strict target admission. */ }
        }
        assertEquals(256, QcBrokerTarget.parse(new JSONObject().put("source", "demo")
            .put("sessionId", "x".repeat(256)).put("generation", 1)).sessionId.length());
    }
}
