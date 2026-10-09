package org.freevia.qcremote;

import android.Manifest;
import android.app.AlertDialog;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.Uri;
import android.os.Build;
import androidx.core.content.ContextCompat;
import com.getcapacitor.JSObject;
import com.getcapacitor.PermissionState;
import com.getcapacitor.Plugin;
import com.getcapacitor.PluginCall;
import com.getcapacitor.PluginMethod;
import com.getcapacitor.annotation.CapacitorPlugin;
import com.getcapacitor.annotation.Permission;
import com.getcapacitor.annotation.PermissionCallback;
import java.io.IOException;
import okhttp3.Call;
import okhttp3.Callback;
import okhttp3.MediaType;
import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.RequestBody;
import okhttp3.Response;
import org.json.JSONObject;

@CapacitorPlugin(
    name = "QcRelay",
    permissions = @Permission(alias = "notifications", strings = { Manifest.permission.POST_NOTIFICATIONS })
)
public final class QcRelayPlugin extends Plugin {
    private final OkHttpClient http = new OkHttpClient();
    private volatile String relayState = "stopped";
    /** The name a phone pairs under unless the caller names it otherwise. */
    static final String DEFAULT_DEVICE_NAME = "QC Remote on Android";
    /** Ephemeral receiver capability for browser pairing. Never enters the WebView or durable storage. */
    private volatile BrowserPairing browserPairing;
    private volatile boolean browserPairingCompleting;
    private final BroadcastReceiver statusReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            relayState = intent.getStringExtra(QcRelayService.EXTRA_STATE);
            JSObject value = new JSObject(); value.put("state", relayState);
            notifyListeners("relayState", value, true);
        }
    };

    @Override public void load() {
        // The service can remain alive while Android recreates the activity or
        // WebView. Use its live state when present; persisted "connected" is
        // only last-known state and cannot prove a Service/WebSocket exists.
        RelayCredentialStore store = new RelayCredentialStore(getContext());
        relayState = QcRelayService.stateForStatus(QcRelayService.liveState(), store.relayState());
        ContextCompat.registerReceiver(getContext(), statusReceiver,
            new IntentFilter(QcRelayService.ACTION_STATUS), ContextCompat.RECEIVER_NOT_EXPORTED);
        store.clearRetiredAutoApproval();
        backfillOwnerEmail();
    }

    @PluginMethod public void status(PluginCall call) {
        RelayCredentialStore store = new RelayCredentialStore(getContext());
        relayState = QcRelayService.stateForStatus(QcRelayService.liveState(), store.relayState());
        JSObject result = new JSObject();
        result.put("paired", store.paired()); result.put("state", relayState);
        result.put("accessMode", RelayAccessPolicy.mode(getContext()));
        String endpoint = store.endpoint(); if (endpoint != null) result.put("endpoint", endpoint);
        String deviceId = store.deviceId(); if (deviceId != null) result.put("deviceId", deviceId);
        call.resolve(result);
    }

    /** Returns aggregate account usage without ever exposing the host credential to JavaScript. */
    @PluginMethod public void usage(PluginCall call) {
        RelayCredentialStore store = new RelayCredentialStore(getContext());
        String endpoint = normalizedEndpoint(store.endpoint());
        String deviceId = store.deviceId();
        final String credential;
        try { credential = store.credential(); }
        catch (Exception error) { call.reject("Pair this phone with QC Remote Cloud first.", "PAIRING_REQUIRED", error); return; }
        if (endpoint == null || deviceId == null || credential == null) {
            call.reject("Pair this phone with QC Remote Cloud first.", "PAIRING_REQUIRED"); return;
        }
        Request request = new Request.Builder()
            .url(endpoint + "/v1/device/usage?deviceId=" + Uri.encode(deviceId))
            .header("Authorization", "Bearer " + credential).get().build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) {
                call.reject("Could not read relay usage.", "USAGE_NETWORK_ERROR", error);
            }
            @Override public void onResponse(Call ignored, Response response) {
                try (response) {
                    if (!response.isSuccessful()) {
                        call.reject(response.code() == 401 || response.code() == 403
                            ? "QC Remote Cloud no longer recognizes this phone. Pair it again."
                            : "QC Remote Cloud could not read relay usage.", "USAGE_REJECTED");
                        return;
                    }
                    JSONObject payload = new JSONObject(response.body().string());
                    JSObject result = new JSObject();
                    result.put("day", payload.getString("day"));
                    result.put("commands", payload.getLong("commands"));
                    if (payload.has("lastCommandAt") && !payload.isNull("lastCommandAt")) result.put("lastCommandAt", payload.getLong("lastCommandAt"));
                    if (payload.has("platformUsage") && !payload.isNull("platformUsage")) result.put("platformUsage", payload.getString("platformUsage"));
                    call.resolve(result);
                } catch (Exception error) { call.reject("QC Remote Cloud returned invalid relay usage.", "USAGE_RESPONSE_ERROR", error); }
            }
        });
    }

    /** Navigation only: secure web sign-in and final destructive confirmation remain in the browser. */
    @PluginMethod public void openAccountDeletionPage(PluginCall call) {
        try {
            String url = accountDeletionUrl(call.getString("endpoint"));
            getContext().startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            call.resolve(new JSObject().put("opened", true));
        } catch (Exception error) {
            call.reject("Could not open the account deletion page.", "ACCOUNT_DELETION_PAGE_UNAVAILABLE");
        }
    }

    static String accountDeletionUrl(String rawEndpoint) {
        String endpoint = normalizedEndpoint(rawEndpoint);
        if (endpoint == null || !endpoint.startsWith("https://"))
            throw new IllegalArgumentException("Account deletion requires the secure relay website.");
        return GeneratedRelayProfile.ACCOUNT_DELETION_INFORMATION_URL;
    }

    @PluginMethod public void pair(PluginCall call) {
        if (needsNotificationPermission()) {
            explainNotificationPermission(call, "pairNotificationPermission");
            return;
        }
        beginPair(call);
    }

    /**
     * Starts the one-click browser flow. The signed-in browser turns this
     * short-lived, native-only receiver capability into a one-time device
     * pairing code; JavaScript only receives the URL it should open.
     */
    @PluginMethod public void beginBrowserPairing(PluginCall call) {
        if (needsNotificationPermission()) {
            explainNotificationPermission(call, "beginBrowserPairingNotificationPermission");
            return;
        }
        beginBrowserPairingAfterPermission(call);
    }

    @PermissionCallback private void beginBrowserPairingNotificationPermission(PluginCall call) {
        if (needsNotificationPermission()) {
            call.reject("Notification permission is required while the remote relay is active.", "NOTIFICATION_PERMISSION_DENIED");
            return;
        }
        beginBrowserPairingAfterPermission(call);
    }

    private void beginBrowserPairingAfterPermission(PluginCall call) {
        String endpoint = normalizedEndpoint(call.getString("endpoint", ""));
        if (endpoint == null) { call.reject("A secure HTTPS relay is required.", "INVALID_PAIRING"); return; }
        Request request = new Request.Builder().url(endpoint + "/v1/host/pairing-sessions")
            .post(RequestBody.create(pairingSessionBody(DEFAULT_DEVICE_NAME).toString(),
                MediaType.get("application/json; charset=utf-8"))).build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) {
                call.reject("Could not reach the relay.", "PAIRING_NETWORK_ERROR", error);
            }
            @Override public void onResponse(Call ignored, Response response) {
                try (response) {
                    if (!response.isSuccessful()) { call.reject("Could not start secure pairing.", "PAIRING_REJECTED"); return; }
                    JSONObject payload = new JSONObject(response.body().string());
                    String sessionId = payload.getString("sessionId");
                    String receiverSecret = payload.getString("receiverSecret");
                    String pairUrl = payload.getString("pairUrl");
                    long expiresAt = payload.getLong("expiresAt");
                    if (sessionId.isEmpty() || receiverSecret.length() < GeneratedRelayProfile.MINIMUM_CREDENTIAL_LENGTH
                        || !pairUrl.startsWith(endpoint + "/pair")) throw new IllegalStateException("Relay returned an invalid pairing session.");
                    browserPairing = new BrowserPairing(endpoint, sessionId, receiverSecret, expiresAt);
                    browserPairingCompleting = false;
                    Intent browser = new Intent(Intent.ACTION_VIEW, Uri.parse(pairUrl)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                    try { getContext().startActivity(browser); }
                    catch (Exception error) { browserPairing = null; throw new IllegalStateException("No browser is available to sign in.", error); }
                    JSObject result = new JSObject(); result.put("expiresAt", expiresAt); result.put("opened", true); call.resolve(result);
                } catch (Exception error) { call.reject("Could not start secure browser pairing.", "PAIRING_START_ERROR", error); }
            }
        });
    }

    /** What this phone declares about itself when it opens a pairing session.
     *
     * The relay's confirmation page shows it beside the Google account, so the
     * person can tell the phone in their hand from a pairing link someone else
     * started. It is a declaration, not proof, and the page says so. The label
     * is the name this phone registers under when the pairing completes, never
     * the hardware model, which this app does not disclose; bounded to the
     * relay's 80 characters. */
    static JSONObject pairingSessionBody(String deviceName) {
        String label = deviceName == null ? "" : deviceName.trim();
        if (label.isEmpty()) label = DEFAULT_DEVICE_NAME;
        if (label.length() > 80) label = label.substring(0, 80).trim();
        try { return new JSONObject().put("label", label).put("platform", "android"); }
        catch (org.json.JSONException impossible) { throw new IllegalStateException(impossible); }
    }

    /** Polls only while the app owns an in-memory pairing session; no code is shown or copied. */
    @PluginMethod public void pollBrowserPairing(PluginCall call) {
        BrowserPairing pending = browserPairing;
        if (pending == null) { call.reject("Start browser pairing first.", "PAIRING_REQUIRED"); return; }
        if (pending.expiresAt < System.currentTimeMillis()) {
            browserPairing = null; call.reject("The browser pairing session expired. Start it again.", "PAIRING_EXPIRED"); return;
        }
        if (browserPairingCompleting) { JSObject result = new JSObject(); result.put("state", "pairing"); call.resolve(result); return; }
        Request request = new Request.Builder().url(pending.endpoint + "/v1/host/pairing-sessions/" + pending.sessionId)
            .header("Authorization", "Bearer " + pending.receiverSecret).get().build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) { call.reject("Could not check browser pairing.", "PAIRING_NETWORK_ERROR", error); }
            @Override public void onResponse(Call ignored, Response response) {
                try (response) {
                    if (response.code() == 202) { JSObject result = new JSObject(); result.put("state", "waiting"); result.put("expiresAt", pending.expiresAt); call.resolve(result); return; }
                    if (!response.isSuccessful()) { call.reject("Browser pairing was rejected.", "PAIRING_REJECTED"); return; }
                    JSONObject payload = new JSONObject(response.body().string());
                    String pairingCode = payload.getString("pairingCode");
                    if (pairingCode.isEmpty()) throw new IllegalStateException("Relay returned an invalid browser pairing result.");
                    browserPairingCompleting = true;
                    pairWithCode(pending.endpoint, pairingCode, "QC Remote on Android", call, true);
                } catch (Exception error) { call.reject("Could not complete browser pairing.", "PAIRING_STORAGE_ERROR", error); }
            }
        });
    }

    @PermissionCallback private void pairNotificationPermission(PluginCall call) {
        if (needsNotificationPermission()) {
            call.reject("Notification permission is required while the remote relay is active.", "NOTIFICATION_PERMISSION_DENIED");
            return;
        }
        beginPair(call);
    }

    private void beginPair(PluginCall call) {
        String endpoint = normalizedEndpoint(call.getString("endpoint", ""));
        String pairingCode = call.getString("pairingCode", "").trim();
        String deviceName = call.getString("deviceName", "QC Remote on Android").trim();
        if (endpoint == null || deviceName.isEmpty()
            || pairingCode.length() < GeneratedRelayProfile.PAIRING_CODE_MINIMUM_LENGTH
            || pairingCode.length() > GeneratedRelayProfile.PAIRING_CODE_MAXIMUM_LENGTH) {
            call.reject("A secure HTTPS relay and valid pairing code are required.", "INVALID_PAIRING"); return;
        }
        pairWithCode(endpoint, pairingCode, deviceName, call, false);
    }

    private void pairWithCode(String endpoint, String pairingCode, String deviceName, PluginCall call, boolean fromBrowser) {
        try {
            RelayCredentialStore pairingStore = new RelayCredentialStore(getContext());
            final RelayCredentialStore.Snapshot heldSnapshot = pairingStore.snapshot();
            Request request = pairingRequest(endpoint,pairingCode,deviceName,heldSnapshot);
            if (fromBrowser) browserPairing=null; // Never redeem the same code again after an ambiguous response.
            pairingHttpClient(http).newCall(request).enqueue(new Callback() {
                @Override public void onFailure(Call ignored, IOException error) {
                    browserPairingCompleting = false;
                    android.util.Log.e("QcRelayPlugin", "Pairing request failed: "
                        + error.getClass().getName() + ": " + error.getMessage(), error);
                    call.reject("Could not reach the relay.", "PAIRING_NETWORK_ERROR", error);
                }
                @Override public void onResponse(Call ignored, Response response) {
                    boolean adopted=false;
                    try (response) {
                        if (!response.isSuccessful()) {
                            browserPairingCompleting = false;
                            String code=pairingResponseError(response.code(),heldSnapshot.paired());
                            call.reject("PAIRING_REPLACEMENT_UNSUPPORTED".equals(code)
                                ? "Relay upgrade required; no legacy pairing fallback was attempted."
                                : "The pairing code was rejected.",code);
                            return;
                        }
                        JSONObject payload = new JSONObject(response.body().string());
                        if (heldSnapshot.paired() && !(payload.opt("previousDeviceReplacementPending") instanceof Boolean))
                            throw new IllegalStateException("Relay did not confirm the versioned replacement contract.");
                        String credential = payload.getString("deviceCredential");
                        String deviceId = payload.getString("deviceId");
                        if (credential.length() < GeneratedRelayProfile.MINIMUM_CREDENTIAL_LENGTH)
                            throw new IllegalStateException("Relay returned an invalid credential.");
                        if (deviceId.trim().isEmpty()) throw new IllegalStateException("Relay returned an invalid device ID.");
                        JSONObject receipt = payload.optJSONObject("pairing");
                        String arriving = receipt == null ? "" : receipt.optString("email", "").trim();
                        RelayCredentialStore store = new RelayCredentialStore(getContext());
                        // Pairing was "last sign-in wins": the relay mints its
                        // offer for whichever Google account signs in, and this
                        // phone stored whatever came back. A pairing link that
                        // reached the wrong person was enough to move the phone
                        // onto their account. Changing hands is now deliberate:
                        // unpair here, then pair.
                        String refusalCode = store.adoptIfCurrent(heldSnapshot, endpoint, credential, deviceId, arriving);
                        String[] refusal = refusalCode == null ? null : new String[] { refusalCode, "The held pairing changed or its owner could not be verified. Unpair explicitly before changing accounts." };
                        if (refusal != null) {
                            browserPairingCompleting = false;
                            withdrawCredential(endpoint, deviceId, credential);
                            call.reject(refusal[1], refusal[0]);
                            return;
                        }
                        adopted=true;
                        if (fromBrowser) browserPairing = null;
                        browserPairingCompleting = false;
                        // Pairing is durable even when the QC is not attached.
                        // The connectedDevice service starts after USB access
                        // is granted, never before Android's runtime prerequisite.
                        if (QcUsbPlugin.relayForegroundServiceEligible()) startService();
                        else relayState = "stopped";
                        JSObject result = new JSObject(); result.put("paired", true); result.put("endpoint", endpoint); result.put("state", "paired");
                        if (payload.opt("previousDeviceReplacementPending") instanceof Boolean)
                            result.put("previousDeviceReplacementPending",payload.getBoolean("previousDeviceReplacementPending"));
                        call.resolve(result);
                    } catch (Exception error) {
                        browserPairingCompleting = false;
                        call.reject(adopted ? "Pairing was saved, but activation did not complete. Restart Remote; do not redeem the same code again."
                            : "Pairing was not confirmed in secure storage. No new connection was authorized; start a fresh browser pairing if needed.",
                            adopted ? "PAIRING_ACTIVATION_ERROR" : "PAIRING_STORAGE_ERROR",error);
                    }
                }
            });
        } catch (Exception error) { browserPairingCompleting = false; call.reject("Pairing request is invalid.", "INVALID_PAIRING", error); }
    }

    @PluginMethod public void start(PluginCall call) {
        if (!new RelayCredentialStore(getContext()).paired()) { call.reject("Pair this phone first.", "PAIRING_REQUIRED"); return; }
        if (needsNotificationPermission()) {
            explainNotificationPermission(call, "startNotificationPermission");
            return;
        }
        startRelay(call);
    }

    @PermissionCallback private void startNotificationPermission(PluginCall call) {
        if (needsNotificationPermission()) {
            call.reject("Notification permission is required while the remote relay is active.", "NOTIFICATION_PERMISSION_DENIED");
            return;
        }
        startRelay(call);
    }

    private void startRelay(PluginCall call) {
        if (!QcUsbPlugin.relayForegroundServiceEligible()) {
            call.reject("Connect the Quad Cortex and grant USB access before starting Remote.", "USB_PERMISSION_REQUIRED");
            return;
        }
        startService();
        call.resolve();
    }

    private boolean needsNotificationPermission() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
            && getPermissionState("notifications") != PermissionState.GRANTED;
    }

    /** Explain the one persistent relay notification before Android shows its system permission prompt. */
    private void explainNotificationPermission(PluginCall call, String callback) {
        if (getActivity() == null) {
            call.reject("Notification permission is required while the remote relay is active.", "NOTIFICATION_PERMISSION_DENIED");
            return;
        }
        getActivity().runOnUiThread(() -> new AlertDialog.Builder(getActivity())
            .setTitle("Allow Remote notifications?")
            .setMessage("Remote access keeps a secure connection to your Quad Cortex while " + getContext().getString(R.string.app_name) + " is in the background. "
                + "Android requires one low-priority, ongoing notification for that connection. USB control does not use notifications.")
            .setNegativeButton("Not now", (dialog, which) -> call.reject(
                "Notification permission is required while the remote relay is active.", "NOTIFICATION_PERMISSION_DENIED"))
            .setPositiveButton("Continue", (dialog, which) -> requestPermissionForAlias("notifications", call, callback))
            .show());
    }
    @PluginMethod public void setAccessMode(PluginCall call) {
        try {
            String mode = call.getString("mode", RelayAccessPolicy.FULL);
            RelayAccessPolicy.setMode(getContext(), mode);
            JSObject result = new JSObject(); result.put("accessMode", mode); call.resolve(result);
        } catch (IllegalArgumentException error) { call.reject(error.getMessage(), "INVALID_ACCESS_MODE", error); }
    }
    /** Whether a new pairing may replace what this phone holds, as
     * {code, message}, or null when it may.
     *
     * An account can only be compared when both sides name one. A legacy
     * pairing stored no account, and treating that as "anything matches" let
     * any sign-in take the phone over, which is the hole the comparison exists
     * to close. Such a pairing keeps working as it is (load() backfills its
     * account from the relay where it can); only replacing it asks for an
     * explicit unpair first, with a message that says so. */
    static String[] pairingOwnerRefusal(boolean paired, String held, String arriving) {
        return pairingOwnerRefusal(paired,held,arriving,false);
    }
    static String[] pairingOwnerRefusal(boolean paired, String held, String arriving, boolean rejected) {
        String code=GeneratedRelayProfile.pairingOwnerRefusal(paired,held,arriving,rejected);
        return code == null ? null : new String[] {code,"Unpair this phone explicitly before replacing a pairing whose account cannot be verified."};
    }

    static String pairingResponseError(int status,boolean replacing) {
        if (replacing && status == 404) return "PAIRING_REPLACEMENT_UNSUPPORTED";
        return "PAIRING_REJECTED";
    }

    static OkHttpClient pairingHttpClient(OkHttpClient base) {
        // Redirected POST bodies must never disclose a predecessor credential.
        return base.newBuilder().followRedirects(false).followSslRedirects(false)
            .retryOnConnectionFailure(false).callTimeout(30,java.util.concurrent.TimeUnit.SECONDS).build();
    }

    static Request pairingRequest(String endpoint,String code,String name,RelayCredentialStore.Snapshot old) throws Exception {
        JSONObject body=pairingRequestBody(endpoint,code,name,old);
        String origin=normalizedEndpoint(endpoint);
        if (origin == null) throw new IllegalArgumentException("Invalid relay origin");
        String path=old != null && old.paired() ? GeneratedRelayProfile.DEVICE_REPLACEMENT_PATH : GeneratedRelayProfile.DEVICE_PAIR_PATH;
        if (old != null && old.paired() && !body.has("previousDeviceCredential"))
            throw new IllegalArgumentException("PAIRING_PREVIOUS_INVALID: Explicitly unpair before replacing this stored identity.");
        return new Request.Builder().url(origin + path)
            .post(RequestBody.create(body.toString(),MediaType.get("application/json; charset=utf-8"))).build();
    }

    static JSONObject pairingRequestBody(String endpoint,String code,String name,RelayCredentialStore.Snapshot old) throws Exception {
        JSONObject body=new JSONObject().put("pairingCode",code).put("deviceName",name).put("protocol",RelayProtocol.VERSION);
        String currentOrigin=normalizedEndpoint(endpoint);
        if (old != null && old.paired()) {
            if (currentOrigin == null || !currentOrigin.equals(normalizedEndpoint(old.endpoint)))
                throw new IllegalArgumentException("PAIRING_ORIGIN_CHANGED: Unpair before changing relay origins.");
            if (!old.rejected && (old.owner == null || old.owner.trim().isEmpty()))
                throw new IllegalArgumentException("PAIRING_OWNER_UNKNOWN: Unpair before replacing an unverifiable account.");
        }
        if (old != null && currentOrigin != null && currentOrigin.equals(normalizedEndpoint(old.endpoint))
            && old.deviceId != null && old.deviceId.matches("^[A-Za-z0-9-]{8,64}$")
            && old.credential != null && old.credential.length() >= 32 && old.credential.length() <= 256) {
            body.put("previousDeviceId",old.deviceId).put("previousDeviceCredential",old.credential);
        }
        return body;
    }

    /** Record the account of a pairing made before accounts were stored.
     *
     * The relay's roster names the account each device was paired to and
     * answers any device holding a live credential, so a legacy pairing can
     * learn its own account without the person doing anything. Nothing waits
     * on it and nothing is cleared when it fails: the pairing goes on working,
     * and pairingOwnerRefusal keeps refusing a replacement until it succeeds. */
    private void backfillOwnerEmail() {
        RelayCredentialStore store = new RelayCredentialStore(getContext());
        if (!store.paired() || !store.ownerEmail().isEmpty()) return;
        final RelayCredentialStore.Snapshot expected;
        try { expected=store.snapshot(); } catch (Exception unavailable) { return; }
        String endpoint = normalizedEndpoint(expected.endpoint);
        String deviceId = expected.deviceId;
        String credential = expected.credential;
        if (endpoint == null || deviceId == null || credential == null) return;
        Request request = new Request.Builder()
            .url(endpoint + GeneratedRelayProfile.DEVICE_ROSTER_PATH + "?deviceId=" + Uri.encode(deviceId))
            .header("Authorization", "Bearer " + credential).get().build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) { }
            @Override public void onResponse(Call ignored, Response response) {
                try (response) {
                    if (!response.isSuccessful()) return;
                    org.json.JSONArray devices = new JSONObject(response.body().string()).optJSONArray("devices");
                    if (devices == null) return;
                    for (int index = 0; index < devices.length(); index++) {
                        JSONObject device = devices.optJSONObject(index);
                        if (device == null || !deviceId.equals(device.optString("deviceId"))) continue;
                        String email = device.optString("email", "").trim();
                        if (!email.isEmpty()) store.backfillOwnerEmail(expected, email);
                        return;
                    }
                } catch (Exception malformed) { }
            }
        });
    }

    /** Hand back a credential this phone has decided not to keep.
     *
     * The relay creates the device record the moment the code is redeemed, so
     * a refused pairing that was simply dropped would leave a live credential
     * for this phone on an account nobody here can reach. Nothing waits on the
     * result: the pairing is refused either way, and an unwithdrawn record
     * expires on its own. */
    private void withdrawCredential(String endpoint, String deviceId, String credential) {
        Request request = new Request.Builder()
            .url(endpoint + GeneratedRelayProfile.DEVICE_REVOKE_PATH + "?deviceId=" + Uri.encode(deviceId))
            .header("Authorization", "Bearer " + credential)
            .post(RequestBody.create(new byte[0], null)).build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) { }
            @Override public void onResponse(Call ignored, Response response) { response.close(); }
        });
    }

    /** Withdraw the pairing at the relay, then locally.
     *
     * Clearing only this phone's store left QC Remote Cloud honouring a
     * credential the person believes they have revoked, for ever: the relay had
     * no idea an unpair had happened. The local clear still runs if the relay
     * cannot be reached, because someone who asked to unpair must never be left
     * paired by a network failure, and the reason is reported so they can retry
     * while the relay still holds it. */
    @PluginMethod public void unpair(PluginCall call) {
        RelayCredentialStore store = new RelayCredentialStore(getContext());
        final RelayCredentialStore.Snapshot unpairSnapshot;
        try { unpairSnapshot=store.snapshot(); }
        catch (Exception unavailable) { call.reject("Could not read the held pairing.","PAIRING_STORAGE_ERROR"); return; }
        // A stored endpoint outside the relay allowlist never receives the
        // credential, not even to revoke it; the local clear still runs.
        String endpoint = normalizedEndpoint(store.endpoint());
        String deviceId = store.deviceId();
        String credential = null;
        try { credential = store.credential(); } catch (Exception ignored) { }
        getContext().startService(new Intent(getContext(), QcRelayService.class).setAction(QcRelayService.ACTION_STOP));
        if (endpoint == null || deviceId == null || credential == null) {
            finishUnpair(call, unpairSnapshot, null);
            return;
        }
        Request request = new Request.Builder()
            .url(endpoint + GeneratedRelayProfile.DEVICE_REVOKE_PATH + "?deviceId=" + Uri.encode(deviceId))
            .header("Authorization", "Bearer " + credential)
            .post(RequestBody.create(new byte[0], null)).build();
        http.newCall(request).enqueue(new Callback() {
            @Override public void onFailure(Call ignored, IOException error) {
                finishUnpair(call, unpairSnapshot, "This phone was unpaired locally, but QC Remote Cloud could not be reached to withdraw the pairing. Unpair again while online.");
            }
            @Override public void onResponse(Call ignored, Response response) {
                try (response) {
                    // A credential the relay no longer recognizes is already withdrawn.
                    boolean withdrawn = response.isSuccessful() || response.code() == 401 || response.code() == 403 || response.code() == 404;
                    finishUnpair(call, unpairSnapshot, withdrawn ? null : "This phone was unpaired locally, but QC Remote Cloud did not confirm the pairing was withdrawn.");
                }
            }
        });
    }

    private void finishUnpair(PluginCall call, RelayCredentialStore.Snapshot expected, String warning) {
        if (!new RelayCredentialStore(getContext()).clearIfCurrent(expected)) {
            call.reject("Pairing changed while unpairing; the new pairing was retained.","PAIRING_SUPERSEDED"); return;
        }
        browserPairing = null; browserPairingCompleting = false; relayState = "stopped";
        JSObject result = new JSObject();
        if (warning != null) result.put("warning", warning);
        call.resolve(result);
    }

    private void startService() {
        ContextCompat.startForegroundService(getContext(), new Intent(getContext(), QcRelayService.class).setAction(QcRelayService.ACTION_START));
    }

    /** The relay endpoint if this build may send a credential there, else null.
     * Applied to what JavaScript asks for and to what an older install stored. */
    static String normalizedEndpoint(String raw) {
        return RelayEndpointPolicy.normalize(raw, BuildConfig.DEBUG);
    }

    private static final class BrowserPairing {
        final String endpoint;
        final String sessionId;
        final String receiverSecret;
        final long expiresAt;
        BrowserPairing(String endpoint, String sessionId, String receiverSecret, long expiresAt) {
            this.endpoint = endpoint; this.sessionId = sessionId; this.receiverSecret = receiverSecret; this.expiresAt = expiresAt;
        }
    }

    @Override protected void handleOnDestroy() {
        try { getContext().unregisterReceiver(statusReceiver); } catch (Exception ignored) {}
        http.dispatcher().executorService().shutdown(); super.handleOnDestroy();
    }
}
