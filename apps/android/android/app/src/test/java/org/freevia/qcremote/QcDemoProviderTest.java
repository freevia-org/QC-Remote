package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import org.json.JSONObject;
import org.junit.Test;

public class QcDemoProviderTest {
    private final AtomicBoolean demoSelected = new AtomicBoolean(true);
    private final AtomicLong selectionRevision = new AtomicLong();
    private final AtomicReference<QcBrokerTarget> brokerTarget = new AtomicReference<>(new QcBrokerTarget("demo", "native-demo", 1));
    private final QcDemoProvider provider = new QcDemoProvider(demoSelected::get, selectionRevision::get, brokerTarget::get);
    private final List<JSONObject> requests = new ArrayList<>();

    private CompletableFuture<JSONObject> invoke() {
        return provider.invoke("device.snapshot", new JSONObject(), new JSONObject(), requests::add);
    }

    private void complete(int index, JSONObject result) {
        JSONObject request = requests.get(index);
        provider.complete(request.optString("id"), request.optString("sessionId"),
            request.optLong("generation"), result, null);
    }

    private static void fails(CompletableFuture<?> value, String code) {
        try { value.join(); fail("Expected refusal"); }
        catch (CompletionException error) { assertEquals(code, ((RelayException) error.getCause()).code); }
    }

    @Test public void returnsTheResultFromTheRegisteredUiSession() throws Exception {
        provider.publish("ui-demo", 7);
        CompletableFuture<JSONObject> result = invoke();
        JSONObject request = requests.get(0);
        assertEquals("ui-demo", request.getString("sessionId"));
        assertEquals(7, request.getLong("generation"));
        assertEquals(1, request.getLong("sequence"));
        assertEquals("device.snapshot", request.getString("method"));
        JSONObject snapshot = new JSONObject().put("presetName", "Demo edit");
        complete(0, snapshot);
        assertSame(snapshot, result.join());
        invoke();
        assertEquals(2, requests.get(1).getLong("sequence"));
    }

    @Test public void onlyAnExplicitPhysicalSelectionRevokesDemoAdmissionAndLateCompletion() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        demoSelected.set(false);
        assertFalse(provider.available());
        fails(invoke(), "NOT_CONNECTED");
        complete(0, new JSONObject());
        fails(before, "DEVICE_IDENTITY_MISMATCH");
        assertEquals(1, requests.size());
    }

    @Test public void aSelectionChangeStillInvalidatesTheOldResultWhenItReturnsToDemo() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        selectionRevision.incrementAndGet();
        complete(0, new JSONObject());
        fails(before, "DEVICE_IDENTITY_MISMATCH");
    }

    @Test public void switchingRegistrationRetiresRequestsWithoutReplayingThem() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        provider.publish("ui-demo", 2);
        fails(before, "DEVICE_IDENTITY_MISMATCH");
        complete(0, new JSONObject());
        CompletableFuture<JSONObject> next = invoke();
        assertFalse(next.isDone());
        assertEquals(2, requests.size());
        complete(1, new JSONObject());
        assertTrue(next.isDone());
    }

    @Test public void backgroundRetiresRequestsAndResumeDoesNotReplayThem() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        provider.foreground(false);
        fails(before, "DEVICE_IDENTITY_MISMATCH");
        fails(invoke(), "NOT_CONNECTED");
        provider.foreground(true);
        complete(0, new JSONObject());
        assertEquals(1, requests.size());
        assertTrue(provider.available());
    }

    @Test public void deadlineReleasesOnlyItsOwnBoundedSlot() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> first = invoke();
        for (int i = 1; i < 8; i++) invoke();
        fails(invoke(), "BUSY");
        provider.expire(first);
        fails(first, "DEVICE_OUTCOME_UNKNOWN");
        assertFalse(provider.canDeliver(requests.get(0).optString("id")));
        assertFalse(invoke().isDone());
        assertEquals(9, requests.size());
    }

    @Test public void wrongSessionCannotConsumeAValidReply() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> result = invoke();
        provider.complete(requests.get(0).optString("id"), "other", 1, new JSONObject(), null);
        provider.complete(requests.get(0).optString("id"), "ui-demo", 9, new JSONObject(), null);
        assertFalse(result.isDone());
        complete(0, new JSONObject());
        assertTrue(result.isDone());
    }

    @Test public void completionCallbacksNeverHoldTheRegistryMonitor() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> result = invoke();
        AtomicBoolean held = new AtomicBoolean(true);
        result.whenComplete((value, error) -> held.set(Thread.holdsLock(provider)));
        provider.publish(null, 2);
        assertFalse(held.get());
    }

    @Test public void beforeEffectAdmissionIsIdempotentAndPinsTheNativeTarget() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> result = invoke();
        JSONObject request = requests.get(0);
        String id = request.optString("id");
        assertTrue(provider.admit(id, "ui-demo", 1, brokerTarget.get()));
        assertTrue(provider.admit(id, "ui-demo", 1, brokerTarget.get()));
        assertFalse(provider.admit(id, "wrong-ui", 1, brokerTarget.get()));
        assertFalse(provider.admit(id, "ui-demo", 2, brokerTarget.get()));
        assertFalse(provider.admit(id, "ui-demo", 1, new QcBrokerTarget("demo", "other", 3)));
        brokerTarget.set(new QcBrokerTarget("demo", "replacement-native-target", 2));
        assertFalse(provider.admit(id, "ui-demo", 1, null));
        assertFalse(provider.canDeliver(id));
        complete(0, new JSONObject());
        fails(result, "DEVICE_IDENTITY_MISMATCH");
    }

    @Test public void expiredOrBackgroundedRequestsCannotPassTheStoreEffectGate() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> expired = invoke();
        String expiredId = requests.get(0).optString("id");
        provider.expire(expired);
        assertFalse(provider.admit(expiredId, "ui-demo", 1, null));
        CompletableFuture<JSONObject> paused = invoke();
        String pausedId = requests.get(1).optString("id");
        provider.foreground(false);
        assertFalse(provider.admit(pausedId, "ui-demo", 1, null));
        provider.foreground(true);
        assertFalse(provider.admit(pausedId, "ui-demo", 1, null));
        fails(paused, "DEVICE_IDENTITY_MISMATCH");
    }
}
