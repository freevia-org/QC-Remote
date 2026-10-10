package org.freevia.qcremote;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

public class QcUsbDevicePolicyTest {
    @Test public void identityRequiresBothPublishedVendorAndSupportedProduct() {
        for (int productId : QcUsbProfile.SUPPORTED_PRODUCT_IDS) {
            assertTrue(QcUsbDevicePolicy.matches(QcUsbProfile.VENDOR_ID, productId));
            assertFalse(QcUsbDevicePolicy.matches(QcUsbProfile.VENDOR_ID + 1, productId));
        }
        assertFalse(QcUsbDevicePolicy.matches(QcUsbProfile.VENDOR_ID, 0));
    }
}
