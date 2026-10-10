package org.freevia.qcremote;

import android.content.Context;
import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** Stores the relay's device credential encrypted by a non-exportable Android Keystore key. */
final class RelayCredentialStore {
    private static final String ALIAS = "qc_control_relay_device_v1";
    private static final String PREFS = "qc_relay_secure";
    private static final String ENDPOINT = "endpoint";
    private static final String CREDENTIAL = "credential";
    private static final String DEVICE_ID = "device_id";
    /** The Google account this phone is paired to, so a later pairing can be
     * told apart from a re-pairing. Not a secret and not a credential: the
     * person saw it on the sign-in page and sees it in the app. */
    private static final String OWNER_EMAIL = "owner_email";
    /** Non-secret last-known service state, used to restore an honest UI after an activity restart. */
    private static final String RELAY_STATE = "relay_state";
    /** Retired phone-local opt-in key, retained only so old installs can clear it. */
    private static final String AUTO_APPROVE_REMOTE_ACTIONS = "auto_approve_remote_actions";

    private static final Object MUTATION_LOCK = new Object();
    private static final String REVISION = "credential_revision";
    private static final String REJECTED = "credential_rejected";
    private static final String PREVIOUS = "previous_pairing_encrypted";
    private static String unconfirmedRevision;
    static void credentialWriteStarted(String revision) { synchronized (MUTATION_LOCK) { unconfirmedRevision=revision; } }
    static void credentialWriteCompleted(String revision,boolean persisted) { synchronized (MUTATION_LOCK) {
        if (persisted && java.util.Objects.equals(unconfirmedRevision,revision)) unconfirmedRevision=null;
    } }
    static boolean credentialRevisionIsDurable(String revision) { synchronized (MUTATION_LOCK) {
        return !java.util.Objects.equals(unconfirmedRevision,revision);
    } }

    static final class Snapshot {
        final String endpoint, credential, deviceId, owner, revision;
        final boolean rejected;
        Snapshot(String endpoint, String credential, String deviceId, String owner, String revision, boolean rejected) {
            this.endpoint=endpoint; this.credential=credential; this.deviceId=deviceId; this.owner=owner; this.revision=revision; this.rejected=rejected;
        }
        boolean paired() { return endpoint != null && credential != null && deviceId != null; }
    }

    Snapshot snapshot() throws Exception {
        synchronized (MUTATION_LOCK) {
            return new Snapshot(endpoint(), credential(), deviceId(), ownerEmail(), prefs().getString(REVISION, "legacy"), prefs().getBoolean(REJECTED, false));
        }
    }
    static boolean sameIdentity(Snapshot a, Snapshot b) {
        return java.util.Objects.equals(a.revision,b.revision) && java.util.Objects.equals(a.endpoint,b.endpoint)
            && java.util.Objects.equals(a.deviceId,b.deviceId) && java.util.Objects.equals(a.credential,b.credential);
    }
    boolean isCurrent(Snapshot expected) throws Exception {
        synchronized (MUTATION_LOCK) { return sameIdentity(expected,snapshot()); }
    }
    boolean markRejected(Snapshot expected,boolean rejected) {
        synchronized (MUTATION_LOCK) {
            try {
                if (!isCurrent(expected)) return false;
                SharedPreferences.Editor edit=prefs().edit().putBoolean(REJECTED,rejected);
                return edit.commit();
            } catch (Exception unavailable) { return false; }
        }
    }
    String previousDeviceId() throws Exception {
        String previous=prefs().getString(PREVIOUS,null);
        return previous == null ? null : new org.json.JSONObject(previous).optString(DEVICE_ID,null);
    }
    boolean acknowledgeReplacement(Snapshot expected,String previousDeviceId) {
        synchronized (MUTATION_LOCK) {
            try {
                if (!isCurrent(expected) || previousDeviceId == null || !previousDeviceId.equals(previousDeviceId())) return false;
                return prefs().edit().remove(PREVIOUS).commit();
            } catch (Exception unavailable) { return false; }
        }
    }
    String adoptIfCurrent(Snapshot expected,String endpoint,String credential,String deviceId,String owner) throws Exception {
        synchronized (MUTATION_LOCK) {
            Snapshot held=snapshot();
            if (!sameIdentity(held,expected)) return "PAIRING_SUPERSEDED";
            String refusal=GeneratedRelayProfile.pairingOwnerRefusal(held.paired(),held.owner,owner,held.rejected);
            if (refusal != null) return refusal;
            save(endpoint,credential,deviceId,owner);
            return null;
        }
    }
    boolean clearIfCurrent(Snapshot expected) {
        synchronized (MUTATION_LOCK) {
            try { if (!isCurrent(expected)) return false; clear(); return true; }
            catch (Exception unavailable) { return false; }
        }
    }

    private final Context context;

    RelayCredentialStore(Context context) { this.context = context.getApplicationContext(); }

    void save(String endpoint, String credential, String deviceId, String ownerEmail) throws Exception {
        synchronized (MUTATION_LOCK) {
        if (deviceId == null || deviceId.trim().isEmpty()) throw new IllegalArgumentException("Relay device ID is required.");
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, key());
        byte[] encrypted = cipher.doFinal(credential.getBytes(StandardCharsets.UTF_8));
        byte[] value = new byte[cipher.getIV().length + encrypted.length];
        System.arraycopy(cipher.getIV(), 0, value, 0, cipher.getIV().length);
        System.arraycopy(encrypted, 0, value, cipher.getIV().length, encrypted.length);
        // Keep the prior encrypted record until an authenticated new connection is acknowledged.
        org.json.JSONObject previous=new org.json.JSONObject();
        for (String field : new String[] {ENDPOINT,DEVICE_ID,CREDENTIAL,OWNER_EMAIL,REVISION}) {
            String prior=prefs().getString(field,null);
            if (prior != null) previous.put(field,prior);
        }
        String revision=java.util.UUID.randomUUID().toString();
        credentialWriteStarted(revision);
        boolean persisted=prefs().edit().putString(PREVIOUS,previous.toString()).putString(ENDPOINT, endpoint).putString(DEVICE_ID, deviceId.trim()).putString(CREDENTIAL,
            Base64.encodeToString(value, Base64.NO_WRAP))
            .putString(OWNER_EMAIL, ownerEmail == null ? "" : ownerEmail.trim())
            .putString(REVISION,revision).putBoolean(REJECTED,false).commit();
        credentialWriteCompleted(revision,persisted);
        if (!persisted) throw new IllegalStateException("Could not persist pairing; no new connection is permitted. Restart and reauthorize if needed.");
        }
    }

    String endpoint() { return prefs().getString(ENDPOINT, null); }

    /** Empty for a pairing made before the account was recorded, which is not
     * the same as no pairing and must not be compared against anything. */
    String ownerEmail() { return prefs().getString(OWNER_EMAIL, ""); }

    String deviceId() { return prefs().getString(DEVICE_ID, null); }

    /** Fill in the account of a legacy pairing, only while the pairing it was
     * read for is still the one stored and still has no account. */
    void backfillOwnerEmail(Snapshot expected, String email) throws Exception {
        synchronized (MUTATION_LOCK) {
            if (email == null || email.trim().isEmpty() || !isCurrent(expected) || !ownerEmail().isEmpty()) return;
            if (!prefs().edit().putString(OWNER_EMAIL, email.trim()).commit())
                throw new IllegalStateException("Could not persist pairing account.");
        }
    }

    String relayState() {
        if (!paired()) return "stopped";
        String value = prefs().getString(RELAY_STATE, "stopped");
        return RelayState.parse(value).wireValue();
    }

    void setRelayState(String state) {
        prefs().edit().putString(RELAY_STATE, state).apply();
    }

    /** Drop the retired opt-in. Confirmation now belongs to the initiating AI
     * client; no stored value may skip or create a paired-device prompt. */
    void clearRetiredAutoApproval() {
        prefs().edit().remove(AUTO_APPROVE_REMOTE_ACTIONS).apply();
    }

    String credential() throws Exception {
        if (!credentialRevisionIsDurable(prefs().getString(REVISION,"legacy")))
            throw new IllegalStateException("Pairing persistence was not confirmed; new connection refused.");
        String encoded = prefs().getString(CREDENTIAL, null);
        if (encoded == null) return null;
        byte[] value = Base64.decode(encoded, Base64.NO_WRAP);
        if (value.length < 13) throw new IllegalStateException("Stored relay credential is invalid.");
        byte[] iv = new byte[12];
        byte[] encrypted = new byte[value.length - 12];
        System.arraycopy(value, 0, iv, 0, 12);
        System.arraycopy(value, 12, encrypted, 0, encrypted.length);
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, iv));
        return new String(cipher.doFinal(encrypted), StandardCharsets.UTF_8);
    }

    boolean paired() { return endpoint() != null && deviceId() != null && prefs().contains(CREDENTIAL); }

    void clear() { synchronized (MUTATION_LOCK) {
        if (!prefs().edit().clear().putString(REVISION,java.util.UUID.randomUUID().toString()).commit())
            throw new IllegalStateException("Could not clear pairing.");
    } }

    private SharedPreferences prefs() { return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE); }

    private SecretKey key() throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (store.containsAlias(ALIAS)) return ((KeyStore.SecretKeyEntry) store.getEntry(ALIAS, null)).getSecretKey();
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(ALIAS,
            KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .build());
        return generator.generateKey();
    }
}
