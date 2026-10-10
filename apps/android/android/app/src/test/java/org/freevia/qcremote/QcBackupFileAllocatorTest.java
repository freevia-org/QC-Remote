package org.freevia.qcremote;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class QcBackupFileAllocatorTest {
    @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test
    public void existingBackupIsPreservedAndNewNamesAdvance() throws Exception {
        File root = temporaryFolder.newFolder("backups");
        File existing = new File(root, "Show.json");
        byte[] original = "existing backup".getBytes(StandardCharsets.UTF_8);
        Files.write(existing.toPath(), original);

        File second = QcBackupFileAllocator.createUniqueFile(root, "Show.json");
        File third = QcBackupFileAllocator.createUniqueFile(root, "Show.json");

        assertEquals("Show (2).json", second.getName());
        assertEquals("Show (3).json", third.getName());
        assertArrayEquals(original, Files.readAllBytes(existing.toPath()));
    }
}
