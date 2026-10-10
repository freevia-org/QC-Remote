package org.freevia.qcremote;

import android.content.ContentValues;
import android.content.Context;
import android.os.Build;
import android.os.Environment;
import android.provider.MediaStore;
import com.getcapacitor.JSObject;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import org.json.JSONObject;

/** Chooses an app-private destination for legacy Android backup exports. */
final class QcBackupStorage {
    static File legacyRoot(File externalDownloads, File internalFiles, String folderName) throws IOException {
        File base = externalDownloads != null ? externalDownloads : internalFiles;
        if (base == null) throw new IOException("Android could not access private storage for the backup.");
        return new File(base, folderName);
    }

    static String legacyNotice() {
        return "This backup is in QC Remote's app-private storage and will be removed if QC Remote is uninstalled.";
    }

    static JSONObject saveDocument(Context context, JSONObject document, String requestedName) throws Exception {
        String safeName = requestedName.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}]", "_").trim();
        if (safeName.isEmpty()) safeName = "QC Device Backup";
        String fileName = safeName.endsWith(".json") ? safeName : safeName + ".json";
        byte[] bytes = document.toString().getBytes(StandardCharsets.UTF_8);
        if (bytes.length > QcUsbProfile.BACKUP_MAXIMUM_DOCUMENT_BYTES)
            throw new Exception("The native backup exceeds the 32 MiB safety limit.");

        String path;
        String exportedName = fileName;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            ContentValues values = new ContentValues();
            values.put(MediaStore.Downloads.DISPLAY_NAME, fileName);
            values.put(MediaStore.Downloads.MIME_TYPE, "application/json");
            values.put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/"
                + context.getString(R.string.download_folder));
            values.put(MediaStore.Downloads.IS_PENDING, 1);
            android.net.Uri uri = QcPendingDocumentExport.write(new QcPendingDocumentExport.Store<android.net.Uri>() {
                @Override public android.net.Uri insert() {
                    return context.getContentResolver().insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
                }
                @Override public OutputStream open(android.net.Uri pending) throws Exception {
                    return context.getContentResolver().openOutputStream(pending, "w");
                }
                @Override public int publish(android.net.Uri pending) {
                    ContentValues ready = new ContentValues();
                    ready.put(MediaStore.Downloads.IS_PENDING, 0);
                    return context.getContentResolver().update(pending, ready, null, null);
                }
                @Override public int delete(android.net.Uri pending) {
                    return context.getContentResolver().delete(pending, null, null);
                }
            }, bytes);
            path = uri.toString();
        } else {
            File root = legacyRoot(context.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS),
                context.getFilesDir(), context.getString(R.string.download_folder));
            if (!root.isDirectory() && !root.mkdirs()) throw new Exception("Android could not create the backup directory.");
            File file = QcBackupFileAllocator.createUniqueFile(root, fileName);
            try (OutputStream output = new FileOutputStream(file)) {
                output.write(bytes);
            } catch (Exception error) {
                if (!file.delete()) error.addSuppressed(new IOException("Android could not remove the incomplete backup file."));
                throw error;
            }
            path = file.getAbsolutePath();
            exportedName = file.getName();
        }
        JSObject result = new JSObject().put("cancelled", false).put("path", path).put("name", exportedName);
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) result.put("storageNotice", legacyNotice());
        return result;
    }

    private QcBackupStorage() {}
}
