package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import org.json.JSONObject;
import org.junit.Test;

public final class QcPendingOperationsTest {
    @Test public void startupErrorFailsAWaitingReadyAtOnceWithItsCause() throws Exception {
        QcPendingOperations operations = new QcPendingOperations();
        CompletableFuture<JSONObject> result = new CompletableFuture<>();
        QcPendingOperations.Entry<String> ready = operations.register("ready", result);
        assertTrue(operations.fail(ready, new RelayException("USB_CONNECT_FAILED",
            "QC lifecycle initializing: Protocol(ModuleStatisticsDataError)")));
        assertTrue("the open future must not wait for its deadline", result.isDone());
        try { result.get(); fail("expected failure"); }
        catch (ExecutionException error) {
            assertTrue(error.getCause().getMessage().contains("Protocol(ModuleStatisticsDataError)"));
            assertTrue(error.getCause() instanceof RelayException);
        }
        assertTrue(operations.isEmpty());
    }

    @Test public void failingAnAlreadyCompletedEntryIsANoOp() {
        QcPendingOperations operations = new QcPendingOperations();
        CompletableFuture<JSONObject> result = new CompletableFuture<>();
        QcPendingOperations.Entry<String> ready = operations.register("ready", result);
        assertTrue(operations.remove(ready));
        result.complete(new JSONObject());
        assertFalse(operations.fail(ready, new RuntimeException("late")));
        assertFalse(result.isCompletedExceptionally());
    }
}
