package org.freevia.qcremote;

import android.util.AtomicFile;
import com.getcapacitor.JSObject;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import org.json.JSONObject;

/** App-private, atomic persistence for the shared TypeScript demo device. */
@CapacitorPlugin(name = "DemoDeviceStorage")
public final class DemoDeviceStoragePlugin extends Plugin {
    private static final int MAXIMUM_STORE_BYTES = 64 * 1024 * 1024;

    private File storePath() {
        return new File(new File(getContext().getFilesDir(), "virtual-device"), "store.json");
    }

    @PluginMethod
    public void read(PluginCall call) {
        try {
            AtomicFile file = new AtomicFile(storePath());
            JSObject result = new JSObject();
            if (!file.getBaseFile().exists()) {
                result.put("text", JSONObject.NULL);
            } else {
                if (file.getBaseFile().length() > MAXIMUM_STORE_BYTES) {
                    call.reject("The saved demo device document is too large to open.");
                    return;
                }
                byte[] bytes = file.readFully();
                result.put("text", new String(bytes, StandardCharsets.UTF_8));
            }
            call.resolve(result);
        } catch (Exception error) {
            call.reject("Could not read the demo device: " + error.getMessage(), error);
        }
    }

    @PluginMethod
    public void write(PluginCall call) {
        String text = call.getString("text");
        if (text == null) {
            call.reject("The demo device document is missing.");
            return;
        }
        byte[] bytes = text.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAXIMUM_STORE_BYTES) {
            call.reject("The demo device document is too large to save.");
            return;
        }
        try {
            JSONObject document = new JSONObject(text);
            if (document.optInt("version", -1) != 1) {
                call.reject("The demo device document is not a version 1 virtual device document.");
                return;
            }
            File path = storePath();
            File directory = path.getParentFile();
            if (directory == null || (!directory.isDirectory() && !directory.mkdirs())) {
                call.reject("Could not create the demo device folder.");
                return;
            }
            AtomicFile file = new AtomicFile(path);
            FileOutputStream stream = null;
            try {
                stream = file.startWrite();
                stream.write(bytes);
                file.finishWrite(stream);
            } catch (Exception error) {
                if (stream != null) file.failWrite(stream);
                throw error;
            }
            call.resolve();
        } catch (Exception error) {
            call.reject("Could not save the demo device: " + error.getMessage(), error);
        }
    }
}
