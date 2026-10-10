package org.freevia.qcremote;

import android.content.Intent;
import android.hardware.usb.UsbManager;
import android.os.Bundle;
import com.getcapacitor.BridgeActivity;

public class MainActivity extends BridgeActivity {
    @Override
    public void onCreate(Bundle savedInstanceState) {
        registerPlugin(QcUsbPlugin.class);
        registerPlugin(QcRelayPlugin.class);
        registerPlugin(ScreenWakePlugin.class);
        registerPlugin(DemoDeviceStoragePlugin.class);
        super.onCreate(savedInstanceState);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }

    // BridgeActivity widens onResume to public; matching it is required.
    @Override
    public void onResume() {
        super.onResume();
        consumeUsbAttachment();
    }

    /** Being launched on attachment is itself the grant.
     *
     * Android shows its own "open this app for this device" confirmation for
     * the manifest's USB_DEVICE_ATTACHED filter, and starting us is how it
     * reports that the person said yes. That answer reaches the app only as
     * this intent - there is no permission broadcast for it - so an activity
     * that never read its own intent threw the grant away, and the plugin
     * asked again. Reading it here is what makes one plug raise one dialog.
     *
     * The action is cleared once consumed so a later resume cannot replay it. */
    private void consumeUsbAttachment() {
        Intent intent = getIntent();
        if (intent == null) return;
        if (!UsbManager.ACTION_USB_DEVICE_ATTACHED.equals(intent.getAction())) return;
        intent.setAction(null);
        setIntent(intent);
        QcUsbPlugin.systemGrantedAttachment();
    }
}
