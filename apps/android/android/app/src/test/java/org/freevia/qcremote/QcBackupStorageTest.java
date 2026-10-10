package org.freevia.qcremote;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import java.io.File;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import org.junit.Test;

public class QcBackupStorageTest {
    private static final class FakeStore implements QcPendingDocumentExport.Store<String> {
        String inserted;
        String deleted;
        int publishedRows = 1;
        int deletedRows = 1;
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
