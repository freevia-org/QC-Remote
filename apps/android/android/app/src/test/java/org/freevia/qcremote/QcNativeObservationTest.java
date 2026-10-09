package org.freevia.qcremote;

import static org.junit.Assert.*;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import org.junit.Test;

public class QcNativeObservationTest {
    @Test public void ticketAndPayloadAreSeparatedWithoutLoadingJni() throws Exception {
        String ticket = "{\"connection\":1,\"sequence\":2,\"epoch\":{\"sessionId\":\"test-\u00e9\",\"revision\":3}}";
        String header = "{\"observation\":" + ticket
            + ",\"states\":[{\"kind\":\"position\",\"position\":4}],\"tempoClock\":null,"
            + "\"lifecycle\":{\"phase\":\"initializing\",\"sessionValidated\":true,\"error\":null}}";
        byte[] identity = header.getBytes(StandardCharsets.UTF_8);
        byte[] payload = new byte[] {0, (byte) 0xff, 8, 1};
        byte[] packet = ByteBuffer.allocate(6 + identity.length + payload.length)
            .order(ByteOrder.LITTLE_ENDIAN).putShort((short) 50000).putInt(identity.length)
            .put(identity).put(payload).array();
        QcNativeStateDecoder.DecodedFrame frame = QcNativeStateDecoder.DecodedFrame.decode(packet);
        assertEquals(50000, frame.messageType);
        assertEquals(1, new org.json.JSONObject(frame.observationTicket).getInt("connection"));
        assertEquals("test-\u00e9", frame.observationFence.sessionId);
        assertEquals(3, frame.observationFence.revision);
        assertEquals(2, frame.observationFence.sequence);
        assertEquals(1, frame.states.size());
        assertEquals(4, frame.states.get(0).optInt("position"));
        assertNull(frame.tempoClock);
        assertTrue(frame.lifecycle.sessionValidated);
        assertFalse(frame.observationTicket.contains("states"));
        assertArrayEquals(payload, frame.payload);
    }

    @Test public void malformedNativeLengthsCannotBecomePayload() {
        assertNull(QcNativeStateDecoder.DecodedFrame.decode(new byte[0]));
        assertThrows(IllegalStateException.class, () -> QcNativeStateDecoder.DecodedFrame.decode(new byte[5]));
        for (int length : new int[] {0, 1, -1, Integer.MAX_VALUE}) {
            byte[] packet = ByteBuffer.allocate(6).order(ByteOrder.LITTLE_ENDIAN)
                .putShort((short) 15).putInt(length).array();
            assertThrows(IllegalStateException.class, () -> QcNativeStateDecoder.DecodedFrame.decode(packet));
        }
    }

    @Test public void publicationRetainsNativeOrderVersionAndSessionReceipt() throws Exception {
        org.json.JSONObject value = new org.json.JSONObject("{\"observation\":{\"connection\":2,\"sequence\":3},"
            + "\"observedAt\":123456,\"states\":[{\"kind\":\"position\",\"position\":4,"
            + "\"stateVersion\":{\"session\":\"native-session\",\"revision\":9}}],"
            + "\"tempoClock\":{\"bpm\":120}}");
        QcNativeStateDecoder.StatePublication publication = new QcNativeStateDecoder.StatePublication(value);
        assertEquals(2, new org.json.JSONObject(publication.observationTicket).getInt("connection"));
        assertEquals(123456, publication.observedAt);
        assertEquals(4, publication.states.get(0).getInt("position"));
        assertEquals(9, publication.states.get(0).getJSONObject("stateVersion").getInt("revision"));
        assertEquals("native-session", publication.states.get(0).getJSONObject("stateVersion").getString("session"));
        assertEquals(120, publication.tempoClock.getInt("bpm"));
    }
}
