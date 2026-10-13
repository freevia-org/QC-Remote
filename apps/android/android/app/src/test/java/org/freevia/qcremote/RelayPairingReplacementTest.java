package org.freevia.qcremote;

import static org.junit.Assert.*;
import org.junit.Test;
import org.json.JSONObject;

/** Exercises production request and callback admission seams; no network or Android keystore. */
public class RelayPairingReplacementTest {
    private static RelayCredentialStore.Snapshot held(String revision,String credential,String owner,boolean rejected) {
        return new RelayCredentialStore.Snapshot("https://qcrc.freevia.org",credential,"fixture-device",owner,revision,rejected);
    }
    @Test public void replacementClientNeverFollowsRedirectsWithStoredCredentials() {
        okhttp3.OkHttpClient client=QcRelayPlugin.pairingHttpClient(new okhttp3.OkHttpClient());
        assertFalse(client.followRedirects());
        assertFalse(client.followSslRedirects());
        assertFalse(client.retryOnConnectionFailure());
        assertEquals(30000,client.callTimeoutMillis());
    }
    @Test public void replacementRequestBindsPriorCredentialToOriginAndOwner() throws Exception {
        RelayCredentialStore.Snapshot prior=held("one","x".repeat(32),"owner@example.test",false);
        okhttp3.Request request=QcRelayPlugin.pairingRequest(prior.endpoint,"fixture-code","fixture",prior);
        assertEquals(prior.endpoint + GeneratedRelayProfile.DEVICE_REPLACEMENT_PATH,request.url().toString());
        assertEquals("POST",request.method());
        assertEquals("PAIRING_REPLACEMENT_UNSUPPORTED",QcRelayPlugin.pairingResponseError(404,true));
        assertEquals("PAIRING_REJECTED",QcRelayPlugin.pairingResponseError(404,false));
        okio.Buffer captured=new okio.Buffer(); request.body().writeTo(captured);
        JSONObject body=new JSONObject(captured.readUtf8());
        assertEquals(prior.deviceId,body.getString("previousDeviceId"));
        assertEquals(prior.credential,body.getString("previousDeviceCredential"));
        assertThrows(IllegalArgumentException.class,()->QcRelayPlugin.pairingRequest("https://other.example","fixture-code","fixture",prior));
        assertThrows(IllegalArgumentException.class,()->QcRelayPlugin.pairingRequest(prior.endpoint,"fixture-code","fixture",held("one",prior.credential,"",false)));
        okhttp3.Request ordinary=QcRelayPlugin.pairingRequest(prior.endpoint,"fixture-code","fixture",null);
        assertEquals(prior.endpoint + GeneratedRelayProfile.DEVICE_PAIR_PATH,ordinary.url().toString());
        okio.Buffer ordinaryBody=new okio.Buffer(); ordinary.body().writeTo(ordinaryBody);
        assertFalse(new JSONObject(ordinaryBody.readUtf8()).has("previousDeviceCredential"));
    }
    @Test public void staleIdentityOrConnectionCannotRecordCredentialRejection() {
        RelayCredentialStore.Snapshot prior=held("one","fixture-one","owner@example.test",false);
        assertTrue(QcRelayService.currentConnection(2,2,prior,prior));
        assertFalse(QcRelayService.currentConnection(1,2,prior,prior));
        assertFalse(QcRelayService.currentConnection(2,2,prior,held("two","fixture-one","owner@example.test",false)));
        assertFalse(QcRelayService.currentConnection(2,2,prior,held("one","fixture-two","owner@example.test",false)));
    }
    @Test public void replacementAcknowledgementRequiresExactPriorIdentityAndBooleanFalse() throws Exception {
        JSONObject frame=new JSONObject().put("type",GeneratedRelayProfile.REPLACEMENT_ACK_FRAME_TYPE)
            .put("previousDeviceId","fixture-old").put("previousCredentialAccepted",false);
        assertTrue(QcRelayService.replacementAckMatches(frame,"fixture-old"));
        assertFalse(QcRelayService.replacementAckMatches(frame,"other-old"));
        assertFalse(QcRelayService.replacementAckMatches(frame,null));
        frame.put("previousCredentialAccepted","false");
        assertFalse(QcRelayService.replacementAckMatches(frame,"fixture-old"));
        frame.put("previousCredentialAccepted",true);
        assertFalse(QcRelayService.replacementAckMatches(frame,"fixture-old"));
    }
    private static okhttp3.Response handshake(String url, int code) {
        return new okhttp3.Response.Builder().request(new okhttp3.Request.Builder().url(url).build())
            .protocol(okhttp3.Protocol.HTTP_1_1).code(code).message("fixture").build();
    }
    @Test public void authenticatedHandshakeMatchesTheActualInitialAndReplacementRequestUrls() {
        RelayCredentialStore.Snapshot identity=held("new","x".repeat(32),"owner@example.test",false);
        String initial=QcRelayService.relaySocketUrl(identity.endpoint,identity.deviceId,null);
        String replacement=QcRelayService.relaySocketUrl(identity.endpoint,identity.deviceId,"fixture-old");
        assertEquals(initial + "&" + GeneratedRelayProfile.REPLACEMENT_ACK_QUERY_PARAMETER + "=1",replacement);
        assertTrue(QcRelayService.connectResponseMatches(identity,handshake(initial,101),null));
        assertTrue(QcRelayService.connectResponseMatches(identity,handshake(replacement,101),"fixture-old"));
        // A 401/403 handshake for the exact replacement request must be eligible
        // for credential-rejection handling, rather than endlessly reconnecting.
        assertTrue(QcRelayService.connectResponseMatches(identity,handshake(replacement,401),"fixture-old"));
        assertTrue(QcRelayService.connectResponseMatches(identity,handshake(replacement,403),"fixture-old"));
        assertFalse(QcRelayService.connectResponseMatches(identity,handshake(initial,101),"fixture-old"));
        assertFalse(QcRelayService.connectResponseMatches(identity,handshake(replacement,101),null));
        assertFalse(QcRelayService.connectResponseMatches(identity,handshake(replacement.replace("qcrc.freevia.org","other.example"),101),"fixture-old"));
        assertFalse(QcRelayService.connectResponseMatches(identity,handshake(replacement.replace("deviceId=fixture-device","deviceId=other-device"),101),"fixture-old"));
        assertFalse(QcRelayService.connectResponseMatches(identity,handshake(replacement.replace("replacementAck=1","replacementAck=0"),101),"fixture-old"));
        assertFalse(QcRelayService.connectResponseMatches(identity,null,"fixture-old"));
    }
    @Test public void durableNewIdentitySupersedesAnAlreadyConnectedSocket() {
        RelayCredentialStore.Snapshot old=held("one","fixture-one","owner@example.test",false);
        assertFalse(QcRelayService.shouldConnectOnStart("connected",true,old,old));
        assertTrue(QcRelayService.shouldConnectOnStart("connected",true,old,held("two","fixture-two","owner@example.test",false)));
    }
    @Test public void failedPreferenceCommitCannotAuthorizeANewConnection() {
        String revision="synthetic-failed-write";
        RelayCredentialStore.credentialWriteStarted(revision);
        try {
            RelayCredentialStore.credentialWriteCompleted(revision,false);
            assertFalse(RelayCredentialStore.credentialRevisionIsDurable(revision));
            assertTrue(RelayCredentialStore.credentialRevisionIsDurable("previous-durable"));
            RelayCredentialStore.credentialWriteCompleted("stale-other-write",true);
            assertFalse(RelayCredentialStore.credentialRevisionIsDurable(revision));
        } finally { RelayCredentialStore.credentialWriteCompleted(revision,true); }
        assertTrue(RelayCredentialStore.credentialRevisionIsDurable(revision));
    }
    @Test public void sharedOwnerPolicyAllowsOnlyAuthenticatedRejectionBypass() {
        assertEquals("PAIRING_OWNER_CHANGED",GeneratedRelayProfile.pairingOwnerRefusal(true,"one@example.test","two@example.test",false));
        assertEquals("PAIRING_OWNER_UNKNOWN",GeneratedRelayProfile.pairingOwnerRefusal(true,"","two@example.test",false));
        assertNull(GeneratedRelayProfile.pairingOwnerRefusal(true,"one@example.test","two@example.test",true));
        assertFalse(GeneratedRelayProfile.pairingRejectionHttp(500));
        assertFalse(GeneratedRelayProfile.pairingRejectionClose(1000));
    }
    @Test public void overlappingBrowserPollsCannotBothRedeemTheSameSession() {
        QcRelayPlugin.BrowserPairing session=new QcRelayPlugin.BrowserPairing("https://qcrc.freevia.org","fixture-session","fixture-secret",Long.MAX_VALUE);
        assertTrue(session.claimPoll());
        assertFalse(session.claimPoll());
        // A waiting or failed HTTP poll releases its claim. A successful poll
        // retains the claim until this entire session is consumed.
        session.releasePoll();
        assertTrue(session.claimPoll());
        assertFalse(session.claimPoll());
        QcRelayPlugin.BrowserPairing next=new QcRelayPlugin.BrowserPairing("https://qcrc.freevia.org","next-session","next-secret",Long.MAX_VALUE);
        assertTrue(next.claimPoll());
        assertFalse(session.claimPoll());
    }
}
