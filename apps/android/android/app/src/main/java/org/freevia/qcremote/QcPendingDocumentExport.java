package org.freevia.qcremote;

import java.io.IOException;
import java.io.OutputStream;

/** Owns the inserted pending document until its bytes are written and published. */
final class QcPendingDocumentExport {
    interface Store<T> {
        T insert() throws Exception;
        OutputStream open(T document) throws Exception;
        int publish(T document) throws Exception;
        int delete(T document) throws Exception;
    }

    static <T> T write(Store<T> store, byte[] bytes) throws Exception {
        T document = store.insert();
        if (document == null) throw new IOException("Android could not create the backup file in Downloads.");
        try {
            try (OutputStream output = store.open(document)) {
                if (output == null) throw new IOException("Android could not open the backup file for writing.");
                output.write(bytes);
            }
            int updated = store.publish(document);
            if (updated != 1) throw new IOException(
                "Android could not publish the completed backup file in Downloads.");
            return document;
        } catch (Exception failure) {
            try {
                if (store.delete(document) != 1)
                    throw new IOException("Android could not remove the incomplete backup file.");
            } catch (Exception cleanupFailure) {
                if (cleanupFailure != failure) failure.addSuppressed(cleanupFailure);
            }
            throw failure;
        }
    }

    private QcPendingDocumentExport() {}
}
