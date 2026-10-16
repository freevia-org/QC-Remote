import assert from 'node:assert/strict';
import test from 'node:test';
import { createCdpSession } from './support/cdp-session.mjs';

class Socket extends EventTarget {
  frames = [];
  closed = false;
  send(text) { this.frames.push(JSON.parse(text)); }
  close() { this.closed = true; }
  frame(value) { this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(value) })); }
}

test('CDP resolves matching responses and session events and clears ownership', async () => {
  const socket = new Socket();
  const session = createCdpSession(socket);
  const command = session.send('Page.enable', {}, 'one');
  const loaded = session.wait('Page.loadEventFired', 'one');
  socket.frame({ method: 'Page.loadEventFired', sessionId: 'other' });
  assert.equal(session.pendingCount, 2);
  socket.frame({ id: socket.frames[0].id, result: { enabled: true } });
  socket.frame({ method: 'Page.loadEventFired', sessionId: 'one' });
  assert.deepEqual(await command, { enabled: true });
  await loaded;
  assert.equal(session.pendingCount, 0);
  session.dispose();
  assert.equal(socket.closed, true);
});

test('missing CDP replies and load events reject within their deadlines', async () => {
  const socket = new Socket();
  const session = createCdpSession(socket, { timeoutMilliseconds: 25 });
  try {
    await Promise.all([
      assert.rejects(session.send('Runtime.evaluate'), /Runtime.evaluate timed out/),
      assert.rejects(session.wait('Page.loadEventFired', 'one'), /Page.loadEventFired timed out/),
    ]);
    assert.equal(session.pendingCount, 0);
  } finally { session.dispose(); }
});

for (const event of ['close', 'error', 'dispose', 'malformed']) {
  test(`CDP ${event} rejects every pending command/event and admits no new work`, async () => {
    const socket = new Socket();
    const session = createCdpSession(socket);
    const pending = [assert.rejects(session.send('Page.enable')), assert.rejects(session.wait('Page.loadEventFired', 'one'))];
    if (event === 'dispose') session.dispose();
    else if (event === 'malformed') socket.dispatchEvent(new MessageEvent('message', { data: '{' }));
    else socket.dispatchEvent(new Event(event));
    await Promise.all(pending);
    await assert.rejects(session.send('Page.navigate'));
    assert.equal(session.pendingCount, 0);
    session.dispose();
    assert.equal(socket.closed, true);
  });
}

test('CDP protocol and synchronous send errors do not retain pending timers', async () => {
  const socket = new Socket();
  const session = createCdpSession(socket);
  const response = assert.rejects(session.send('Page.enable'), /rejected/);
  socket.frame({ id: socket.frames[0].id, error: { message: 'rejected' } });
  await response;
  socket.send = () => { throw new Error('send failed'); };
  await assert.rejects(session.send('Page.navigate'), /send failed/);
  assert.equal(session.pendingCount, 0);
  session.dispose();
});
