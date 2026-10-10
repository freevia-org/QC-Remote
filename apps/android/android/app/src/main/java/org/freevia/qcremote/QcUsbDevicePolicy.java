package org.freevia.qcremote;

import android.content.Intent;
import android.hardware.usb.UsbDevice;
import android.hardware.usb.UsbManager;
import android.os.Build;

/** USB attachment parsing and the shared Quad Cortex identity predicate. */
final class QcUsbDevicePolicy {
    @SuppressWarnings("deprecation")
    static UsbDevice deviceExtra(Intent intent) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            return intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice.class);
        }
        return intent.getParcelableExtra(UsbManager.EXTRA_DEVICE);
    }

    static boolean isQuadCortex(UsbDevice candidate) {
        return matches(candidate.getVendorId(), candidate.getProductId());
    }

    static boolean matches(int vendorId, int productId) {
        return vendorId == QcUsbProfile.VENDOR_ID && QcUsbProfile.supportsProductId(productId);
    }

    private QcUsbDevicePolicy() {}
}
