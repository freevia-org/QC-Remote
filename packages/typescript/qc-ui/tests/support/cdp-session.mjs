/** A bounded, disposable CDP connection. Every outstanding wait has one owner. */
export function createCdpSession(socket, { timeoutMilliseconds = 10_000 } = {}) {
  let nextId = 0;
  let closed;
  const requests = new Map();
  const waiters = new Set();
  // Every protocol event, in arrival order, for callers that inspect what the page did.
  const events = [];
  const pending = (collection, key, description) => {
    let entry;
    const promise = new Promise((resolve, reject) => {
      entry = { resolve: value => { clearTimeout(entry.timer); collection.delete(key ?? entry); resolve(value); },
        reject: error => { clearTimeout(entry.timer); collection.delete(key ?? entry); reject(error); } };
      entry.timer = setTimeout(() => entry.reject(new Error(`CDP ${description} timed out`)), timeoutMilliseconds);
    });
    if (key === undefined) collection.add(entry); else collection.set(key, entry);
    return { entry, promise };
  };
  const fail = error => {
    closed ??= error;
    for (const entry of requests.values()) entry.reject(closed);
    for (const entry of waiters) entry.reject(closed);
  };
  const message = event => {
    let frame;
    try { frame = JSON.parse(String(event.data)); } catch { fail(new Error('Invalid CDP JSON')); return; }
    if (frame.id !== undefined) {
      const entry = requests.get(frame.id);
      if (entry) frame.error ? entry.reject(new Error(frame.error.message)) : entry.resolve(frame.result);
    } else {
      events.push({ method: frame.method ?? "", params: frame.params });
      for (const entry of waiters) if (entry.method === frame.method && entry.sessionId === frame.sessionId) entry.resolve();
    }
  };
  const close = () => fail(new Error('CDP socket closed'));
  const error = () => fail(new Error('CDP socket failed'));
  socket.addEventListener('message', message);
  socket.addEventListener('close', close);
  socket.addEventListener('error', error);
  return {
    send(method, params = {}, sessionId) {
      if (closed) return Promise.reject(closed);
      const id = ++nextId;
      const operation = pending(requests, id, method);
      try { socket.send(JSON.stringify({ id, method, params, sessionId })); } catch (error) { operation.entry.reject(error); }
      return operation.promise;
    },
    wait(method, sessionId) {
      if (closed) return Promise.reject(closed);
      const operation = pending(waiters, undefined, method);
      Object.assign(operation.entry, { method, sessionId });
      return operation.promise;
    },
    dispose() {
      fail(new Error('CDP connection disposed'));
      socket.removeEventListener('message', message);
      socket.removeEventListener('close', close);
      socket.removeEventListener('error', error);
      socket.close();
    },
    events,
    get pendingCount() { return requests.size + waiters.size; },
  };
}
