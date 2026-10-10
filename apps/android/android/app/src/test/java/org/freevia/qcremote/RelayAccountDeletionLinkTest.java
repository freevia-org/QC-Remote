package org.freevia.qcremote;
import static org.junit.Assert.*;
import org.junit.Test;
public class RelayAccountDeletionLinkTest {
    @Test public void deletionNavigationUsesTheLiveManualRequestPage() {
        assertEquals("https://freevia.org/qcremote/account/delete/",
            QcRelayPlugin.accountDeletionUrl("https://qcrc.freevia.org"));
        assertThrows(IllegalArgumentException.class, () -> QcRelayPlugin.accountDeletionUrl(null));
        assertThrows(IllegalArgumentException.class, () -> QcRelayPlugin.accountDeletionUrl("javascript:alert(1)"));
    }
}
