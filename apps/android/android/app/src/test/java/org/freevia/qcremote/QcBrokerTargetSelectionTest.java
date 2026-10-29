package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.atomic.AtomicInteger;
import org.json.JSONObject;
import org.junit.Test;

public class QcBrokerTargetSelectionTest {
    private final QcBrokerTarget.Registry targets = new QcBrokerTarget.Registry();
    private final QcDemoProvider provider = new QcDemoProvider(targets::demoSelected, targets::intentRevision, this::current);
    private final List<JSONObject> demoRequests = new ArrayList<>();
    private final AtomicInteger physicalDispatches = new AtomicInteger();
    private boolean physicalReady = true;
    private long usbGeneration = 1;

    private QcBrokerTarget current() {
        QcDemoProvider.Registration registration = provider.registration();
        return targets.selectAvailable(physicalReady, "usb", usbGeneration, provider.available(),
            registration == null ? null : registration.id, registration == null ? 0 : registration.generation);
    }

    /** The same router used by native MCP, with independently counted transports. */
    private CompletableFuture<JSONObject> freshMutation() {
        QcBrokerTarget captured = current();
        return QcBrokerTargetRouter.invoke(targets.demoSelected(), captured,
            () -> provider.invoke("device.setTempo", new JSONObject(), new JSONObject(), demoRequests::add),
            () -> {
                physicalDispatches.incrementAndGet();
                return CompletableFuture.completedFuture(new JSONObject());
            });
    }

    private void choose(String source) {
        targets.commitIntent(source, revision -> true);
        provider.targetChanged();
    }

    private static void refuses(CompletableFuture<?> result, String code) {
        try { result.join(); fail("Expected refusal"); }
        catch (CompletionException error) { assertEquals(code, ((RelayException) error.getCause()).code); }
    }

    private void completeDemo(int index) {
        JSONObject request = demoRequests.get(index);
        provider.complete(request.optString("id"), request.optString("sessionId"), request.optLong("generation"), new JSONObject(), null);
    }

    @Test public void freshMcpMutationNeverFallsThroughToUsbAfterDemoProviderLoss() {
        choose("demo");
        provider.publish("demo-ui", 1);
        CompletableFuture<JSONObject> first = freshMutation();
        assertEquals("demo", current().source);
        completeDemo(0);
        assertTrue(first.isDone());
        provider.publish(null, 2);
        assertNull(current());
        refuses(freshMutation(), "NOT_CONNECTED");
        assertTrue(targets.demoSelected());
        assertEquals(0, physicalDispatches.get());
        provider.publish("replacement-demo", 3);
        CompletableFuture<JSONObject> resumed = freshMutation();
        completeDemo(1);
        assertTrue(resumed.isDone());
        assertEquals(0, physicalDispatches.get());
    }

    @Test public void backgroundAndFailedRegistrationKeepTheDurableDemoIntentUnavailable() {
        choose("demo");
        provider.publish("demo-ui", 1);
        CompletableFuture<JSONObject> pending = freshMutation();
        provider.foreground(false);
        refuses(pending, "DEVICE_IDENTITY_MISMATCH");
        refuses(freshMutation(), "NOT_CONNECTED");
        provider.publish(null, 2);
        provider.foreground(true);
        refuses(freshMutation(), "NOT_CONNECTED");
        assertTrue(targets.demoSelected());
        assertEquals(0, physicalDispatches.get());
    }

    @Test public void automaticUsbAvailabilityAndRecoveryDoNotChangeAnExplicitDemoChoice() {
        choose("demo");
        provider.publish("demo-ui", 1);
        QcBrokerTarget initial = current();
        CompletableFuture<JSONObject> pending = freshMutation();
        physicalReady = false;
        usbGeneration++;
        assertTrue(initial.same(current()));
        physicalReady = true;
        usbGeneration++;
        assertTrue(initial.same(current()));
        completeDemo(0);
        assertTrue(pending.isDone());
        assertEquals("demo", targets.selectedSource());
        assertEquals(0, physicalDispatches.get());
    }

    @Test public void explicitPhysicalSwitchFencesDemoAndRestoresOnlyThePhysicalRoute() {
        choose("demo");
        provider.publish("demo-ui", 1);
        QcBrokerTarget previous = current();
        CompletableFuture<JSONObject> oldRequest = freshMutation();
        choose("physical");
        refuses(oldRequest, "DEVICE_IDENTITY_MISMATCH");
        assertEquals("physical", current().source);
        try { previous.assertCurrent(current()); fail("Old target must be fenced"); }
        catch (RelayException expected) { assertEquals("TARGET_SUPERSEDED", expected.code); }
        assertTrue(freshMutation().isDone());
        assertEquals(1, physicalDispatches.get());
        assertEquals(1, demoRequests.size());
        choose("demo");
        assertFalse(previous.same(current()));
        completeDemo(0);
        CompletableFuture<JSONObject> newRequest = freshMutation();
        assertFalse(newRequest.isDone());
        completeDemo(1);
        assertTrue(newRequest.isDone());
        assertEquals(1, physicalDispatches.get());
    }

    @Test public void pluginRecreationRestoresDemoHoldWithoutNeedingAProvider() {
        targets.restoreIntent("demo", 17);
        assertEquals(17, targets.intentRevision());
        assertEquals("demo", targets.selectedSource());
        refuses(freshMutation(), "NOT_CONNECTED");
        assertEquals(0, physicalDispatches.get());
    }

    @Test public void aFailedDurablePhysicalSwitchCannotReleaseTheDemoHold() {
        choose("demo");
        long before = targets.intentRevision();
        try { targets.commitIntent("physical", revision -> false); fail("Storage failure must refuse the selection"); }
        catch (IllegalStateException expected) { /* No acknowledgement or target switch. */ }
        assertTrue(targets.demoSelected());
        assertEquals(before, targets.intentRevision());
        refuses(freshMutation(), "NOT_CONNECTED");
        assertEquals(0, physicalDispatches.get());
    }

    @Test public void aFailedDemoSaveImmediatelyClosesThePreviousPhysicalRoute() {
        assertEquals("physical", current().source);
        try { targets.commitIntent("demo", revision -> false); fail("Storage failure cannot acknowledge Demo"); }
        catch (IllegalStateException expected) { /* In-memory hold still takes effect. */ }
        assertEquals("demo", targets.selectedSource());
        assertNull(current());
        refuses(freshMutation(), "NOT_CONNECTED");
        assertEquals(0, physicalDispatches.get());
        choose("physical");
        assertTrue(freshMutation().isDone());
        assertEquals(1, physicalDispatches.get());
    }
}
