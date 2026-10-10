package org.freevia.qcremote;

import java.io.File;
import java.io.IOException;
import java.util.Locale;

final class QcBackupFileAllocator {
    private QcBackupFileAllocator() {}

    static File createUniqueFile(File root, String fileName) throws IOException {
        int extensionStart = fileName.toLowerCase(Locale.ROOT).endsWith(".json") ? fileName.length() - 5 : fileName.length();
        String stem = fileName.substring(0, extensionStart);
        String extension = fileName.substring(extensionStart);
        for (int suffix = 1; ; suffix++) {
            String candidateName = suffix == 1 ? fileName : stem + " (" + suffix + ")" + extension;
            File candidate = new File(root, candidateName);
            if (candidate.createNewFile()) return candidate;
            if (suffix == Integer.MAX_VALUE) throw new IOException("Android could not find an unused backup filename.");
        }
    }
}
