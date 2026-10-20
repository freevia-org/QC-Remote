package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import org.json.JSONObject;
import org.junit.Test;

public class QcDemoProviderTest {
    private final AtomicBoolean physical = new AtomicBoolean();
    private final AtomicLong physicalGeneration = new AtomicLong();
    private final QcDemoProvider provider = new QcDemoProvider(physical::get, physicalGeneration::get);
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

    @Test public void physicalAlwaysWinsAdmissionAndLateDemoCompletion() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        physical.set(true);
        assertFalse(provider.available());
        fails(invoke(), "NOT_CONNECTED");
        complete(0, new JSONObject());
        fails(before, "DEVICE_IDENTITY_MISMATCH");
        assertEquals(1, requests.size());
    }

    @Test public void aPhysicalAttachAndDetachStillInvalidatesTheOldResult() {
        provider.publish("ui-demo", 1);
        CompletableFuture<JSONObject> before = invoke();
        physicalGeneration.incrementAndGet();
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
}
