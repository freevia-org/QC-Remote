package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import org.junit.Test;

public class QcBackupStorageTest {
    @Test public void advisoryReportsFollowPersistenceAndNeverModifyNativeDocument() throws Exception {
        org.json.JSONObject document = new org.json.JSONObject("{\"type\":\"backup\",\"creator\":\"quad\",\"payload\":\"AQID\",\"future\":true}");
        String original = document.toString();
        for (String reason : new String[] {"local_match", "local_mismatch", "alternate_whole_payload"}) {
            QcBackupStorage.PreparedBackupDocument prepared = QcBackupStorage.prepareDocument(document);
            java.util.List<String> order = new java.util.ArrayList<>();
            org.json.JSONObject[] durable = new org.json.JSONObject[1];
            org.json.JSONObject result = QcBackupStorage.saveWithReport(prepared,
                value -> {
                    order.add("save");
                    assertEquals(original, new String(value.bytes, java.nio.charset.StandardCharsets.UTF_8));
                    return new org.json.JSONObject().put("path", "public-fixture.json");
                }, saved -> { order.add("record"); durable[0] = saved; }, bytes -> {
                    order.add("inspect");
                    assertTrue(durable[0] != null);
                    return new org.json.JSONObject(GeneratedBackupChecksum.jsonForReason(reason));
                });
            assertEquals(java.util.Arrays.asList("save", "record", "inspect"), order);
            assertTrue(result == durable[0]);
            assertEquals("public-fixture.json", result.getString("path"));
            assertEquals(reason, result.getJSONObject("backupChecksum").getString("reason"));
            assertEquals(original, document.toString());
            assertEquals(original, new String(prepared.bytes, java.nio.charset.StandardCharsets.UTF_8));
            assertTrue(!result.getJSONObject("backupChecksum").has("payload"));
        }
    }

    @Test public void optionalInspectorFailureCannotEraseSavedLocationOrAbortTheCaller() throws Exception {
        QcBackupStorage.PreparedBackupDocument prepared = QcBackupStorage.prepareDocument(new org.json.JSONObject());
        org.json.JSONObject[] durable = new org.json.JSONObject[1];
        org.json.JSONObject result = QcBackupStorage.saveWithReport(prepared,
            value -> new org.json.JSONObject().put("path", "saved-before-report.json"),
            saved -> durable[0] = saved,
            bytes -> { throw new IOException("unavailable inspection"); });
        assertTrue(result == durable[0]);
        assertEquals("saved-before-report.json", result.getString("path"));
        assertEquals("internal_unavailable", result.getJSONObject("backupChecksum").getString("reason"));
        // This returned result remains available to the caller's recovery path.
        QcBackupStorage.attachChecksumReport(result, prepared.bytes, bytes -> { throw new UnsatisfiedLinkError("older host"); });
        assertEquals("saved-before-report.json", result.getString("path"));
    }

    @Test public void malformedReportIsReplacedWithoutLeakingUnexpectedFields() throws Exception {
        org.json.JSONObject saved = new org.json.JSONObject().put("path", "saved.json");
        QcBackupStorage.attachChecksumReport(saved, new byte[0], bytes ->
            new org.json.JSONObject(GeneratedBackupChecksum.jsonForReason("local_match")).put("payload", "must-not-leak"));
        assertEquals("unknown_layout", saved.getJSONObject("backupChecksum").getString("status"));
        assertTrue(!saved.toString().contains("must-not-leak"));
        org.json.JSONObject missingNullable = new org.json.JSONObject(GeneratedBackupChecksum.jsonForReason("alternate_whole_payload"));
        missingNullable.remove("checkedLayout");
        missingNullable.put("payload", "must-not-leak");
        QcBackupStorage.attachChecksumReport(saved, new byte[0], bytes -> missingNullable);
        assertEquals("internal_unavailable", saved.getJSONObject("backupChecksum").getString("reason"));
        assertTrue(!saved.toString().contains("must-not-leak"));
        for (String assurance : new String[] {"authenticityVerified", "stampIntegrityVerified", "metadataIntegrityVerified", "restoreCompatibilityVerified"}) {
            QcBackupStorage.attachChecksumReport(saved, new byte[0], bytes ->
                new org.json.JSONObject(GeneratedBackupChecksum.jsonForReason("local_match")).put(assurance, true));
            assertEquals("internal_unavailable", saved.getJSONObject("backupChecksum").getString("reason"));
        }
    }

    @Test public void storageFailureDoesNotRecordSuccessOrRunInspection() throws Exception {
        QcBackupStorage.PreparedBackupDocument prepared = QcBackupStorage.prepareDocument(new org.json.JSONObject());
        try {
            QcBackupStorage.saveWithReport(prepared,
                value -> { throw new IOException("injected save failure"); },
                saved -> { throw new AssertionError("must not record failed save"); },
                bytes -> { throw new AssertionError("must not inspect failed save"); });
        } catch (IOException expected) {
            assertEquals("injected save failure", expected.getMessage());
            return;
        }
        throw new AssertionError("failed storage must fail before reporting");
    }

    @Test public void aPublishedPendingExportIsRecordedBeforeOptionalInspectionFails() throws Exception {
        org.json.JSONObject document = new org.json.JSONObject("{\"type\":\"backup\",\"future\":true}");
        QcBackupStorage.PreparedBackupDocument prepared = QcBackupStorage.prepareDocument(document);
        FakeStore store = new FakeStore();
        java.util.List<String> order = new java.util.ArrayList<>();
        org.json.JSONObject[] durable = new org.json.JSONObject[1];
        org.json.JSONObject result = QcBackupStorage.saveWithReport(prepared,
            value -> {
                String id = QcPendingDocumentExport.write(store, value.bytes);
                order.add("published");
                return new org.json.JSONObject().put("uri", id).put("name", "Public fixture.json");
            }, saved -> {
                assertEquals(1, store.publishCalls);
                assertEquals(null, store.deleted);
                assertEquals(document.toString(), new String(store.bytes.toByteArray(), java.nio.charset.StandardCharsets.UTF_8));
                durable[0] = saved;
                order.add("recorded");
            }, bytes -> {
                assertTrue(durable[0] != null);
                order.add("inspection");
                throw new IOException("optional checksum bridge unavailable");
            });
        // The caller can continue its existing recovery path with the completed
        // saved result. This is storage/report composition, not a real USB test.
        order.add("caller continued");
        assertEquals(java.util.Arrays.asList("published", "recorded", "inspection", "caller continued"), order);
        assertTrue(result == durable[0]);
        assertEquals("pending-id", result.getString("uri"));
        assertEquals("internal_unavailable", result.getJSONObject("backupChecksum").getString("reason"));
        assertEquals(1, store.publishCalls);
        assertEquals(null, store.deleted);
    }

    @Test public void aPendingPublishFailureCleansItsRowBeforeAnySavedReportCanExist() throws Exception {
        QcBackupStorage.PreparedBackupDocument prepared = QcBackupStorage.prepareDocument(new org.json.JSONObject());
        FakeStore store = new FakeStore();
        store.publishFailure = new IOException("injected pending publish failure");
        try {
            QcBackupStorage.saveWithReport(prepared,
                value -> {
                    String id = QcPendingDocumentExport.write(store, value.bytes);
                    return new org.json.JSONObject().put("uri", id);
                }, saved -> { throw new AssertionError("failed publication must not record success"); },
                bytes -> { throw new AssertionError("failed publication must not run inspection"); });
        } catch (IOException expected) {
            assertTrue(expected == store.publishFailure);
            assertEquals(1, store.publishCalls);
            assertEquals("pending-id", store.deleted);
            return;
        }
        throw new AssertionError("failed pending publication must remain a save failure");
    }

    private static final class FakeStore implements QcPendingDocumentExport.Store<String> {
        String inserted;
        String deleted;
        int publishedRows = 1;
        int deletedRows = 1;
        int publishCalls;
        Exception openFailure;
        Exception publishFailure;
        Exception deleteFailure;
        boolean failWrite;
        final ByteArrayOutputStream bytes = new ByteArrayOutputStream();

        @Override public String insert() { inserted = "pending-id"; return inserted; }
        @Override public OutputStream open(String id) throws Exception {
            if (openFailure != null) throw openFailure;
            if (failWrite) return new OutputStream() {
                @Override public void write(int value) throws IOException { throw new IOException("write failed"); }
            };
            return bytes;
        }
        @Override public int publish(String id) throws Exception {
            publishCalls++;
            if (publishFailure != null) throw publishFailure;
            return publishedRows;
        }
        @Override public int delete(String id) throws Exception {
            deleted = id;
            if (deleteFailure != null) throw deleteFailure;
            return deletedRows;
        }
    }

    @Test public void pendingExportPublishesOnlyAfterWritingAndDoesNotDeleteSuccess() throws Exception {
        FakeStore store = new FakeStore();
        assertEquals("pending-id", QcPendingDocumentExport.write(store, new byte[] {1, 2, 3}));
        assertEquals("pending-id", store.inserted);
        assertEquals(3, store.bytes.size());
        assertEquals(null, store.deleted);
    }

    @Test public void pendingExportDeletesOnlyItsInsertedRowWhenPublishMatchesNoRows() {
        FakeStore store = new FakeStore();
        store.publishedRows = 0;
        try {
            QcPendingDocumentExport.write(store, new byte[] {1});
        } catch (Exception failure) {
            assertTrue(failure.getMessage().contains("publish"));
            assertEquals("pending-id", store.deleted);
            return;
        }
        throw new AssertionError("a publish that updates no rows must fail and clean up");
    }

    @Test public void pendingExportPreservesPublishFailureAndAttachesCleanupFailure() {
        FakeStore store = new FakeStore();
        IOException publishFailure = new IOException("publish unavailable");
        IOException cleanupFailure = new IOException("delete unavailable");
        store.publishFailure = publishFailure;
        store.deleteFailure = cleanupFailure;
        try {
            QcPendingDocumentExport.write(store, new byte[] {1});
        } catch (Exception failure) {
            assertEquals(publishFailure, failure);
            assertEquals(1, failure.getSuppressed().length);
            assertEquals(cleanupFailure, failure.getSuppressed()[0]);
            return;
        }
        throw new AssertionError("publish failure must remain the primary failure");
    }

    @Test public void pendingExportCleansUpWhenWritingFailsAndReportsZeroDeletedRows() {
        FakeStore store = new FakeStore();
        store.failWrite = true;
        store.deletedRows = 0;
        try {
            QcPendingDocumentExport.write(store, new byte[] {1});
        } catch (Exception failure) {
            assertTrue(failure.getMessage().contains("write failed"));
            assertEquals("pending-id", store.deleted);
            assertEquals(1, failure.getSuppressed().length);
            assertTrue(failure.getSuppressed()[0].getMessage().contains("remove"));
            return;
        }
        throw new AssertionError("write failure must cleanup the pending row");
    }

    @Test public void pendingExportCleansUpWhenTheOutputCannotBeOpened() {
        FakeStore store = new FakeStore();
        store.openFailure = new IOException("open unavailable");
        try {
            QcPendingDocumentExport.write(store, new byte[] {1});
        } catch (Exception failure) {
            assertEquals(store.openFailure, failure);
            assertEquals("pending-id", store.deleted);
            return;
        }
        throw new AssertionError("open failure must cleanup the pending row");
    }

    @Test public void legacyStoragePrefersAppSpecificExternalFolder() throws Exception {
        File external = new File("/app/external/Download");
        File internal = new File("/app/files");
        assertEquals(new File(external, "QC Remote"),
            QcBackupStorage.legacyRoot(external, internal, "QC Remote"));
    }

    @Test public void legacyStorageFallsBackToInternalAppFilesWhenExternalIsUnavailable() throws Exception {
        File internal = new File("/app/files");
        assertEquals(new File(internal, "QC Remote"),
            QcBackupStorage.legacyRoot(null, internal, "QC Remote"));
        assertTrue(QcBackupStorage.legacyNotice().contains("app-private storage"));
        assertTrue(QcBackupStorage.legacyNotice().contains("uninstalled"));
    }

    @Test public void legacyStorageFailsClearlyWhenNeitherPrivateLocationExists() {
        try {
            QcBackupStorage.legacyRoot(null, null, "QC Remote");
        } catch (IOException error) {
            assertTrue(error.getMessage().contains("private storage"));
            return;
        }
        throw new AssertionError("missing private storage must not create a relative-path backup");
    }
}
