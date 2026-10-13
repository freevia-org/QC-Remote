package org.freevia.qcremote;

import static org.junit.Assert.*;
import okhttp3.Request;
import okhttp3.WebSocket;
import okio.ByteString;
import org.junit.Test;

public class RelayPeerCloseTest {
    private static final class Socket implements WebSocket {
        int code=-1;
        String reason;
        public Request request() { return new Request.Builder().url("https://qcrc.freevia.org").build(); }
        public long queueSize() { return 0; }
        public boolean send(String text) { return true; }
        public boolean send(ByteString bytes) { return true; }
        public boolean close(int value,String message) { code=value;reason=message;return true; }
        public void cancel() { }
    }
    @Test public void credentialExpiryAndAccountDeletionAcknowledgeAndRetainRejectionEvidence() {
        for (int code:new int[]{4001,4003}) {
            Socket socket=new Socket();int[] actions={0,0,0};
            QcRelayService.handlePeerClosing(socket,code,true,
                ()->actions[0]++,()->actions[1]++,()->actions[2]++);
            assertEquals(code,socket.code);assertEquals("relay closed",socket.reason);
            assertArrayEquals(new int[]{1,1,0},actions);
        }
    }
    @Test public void ordinaryServerCloseStopsReadinessAndExitsConnectedState() {
        Socket socket=new Socket();int[] actions={0,0,0};
        QcRelayService.handlePeerClosing(socket,1000,true,
            ()->actions[0]++,()->actions[1]++,()->actions[2]++);
        assertEquals(1000,socket.code);assertArrayEquals(new int[]{1,0,1},actions);
    }
    @Test public void staleListenerOnlyAcknowledgesItsOwnSocketAndEmptyCloseUsesLegalReplyCode() {
        Socket socket=new Socket();int[] actions={0,0,0};
        QcRelayService.handlePeerClosing(socket,4003,false,
            ()->actions[0]++,()->actions[1]++,()->actions[2]++);
        assertEquals(4003,socket.code);assertArrayEquals(new int[]{0,0,0},actions);
        QcRelayService.handlePeerClosing(socket,1005,false,
            ()->actions[0]++,()->actions[1]++,()->actions[2]++);
        assertEquals(1000,socket.code);assertArrayEquals(new int[]{0,0,0},actions);
    }
}
