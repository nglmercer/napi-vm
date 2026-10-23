const { test } = require('node:test');
const assert = require('node:assert/strict');
// These tests exercise the JS transport without loading a native VM addon.
const bindingPath = require.resolve('../../index.js');
const saved = require.cache[bindingPath];
require.cache[bindingPath] = { id: bindingPath, filename: bindingPath, loaded: true, exports: { Vm: class {} } };
const { VmSession } = require('../../runtime/session.cjs');
if (saved) require.cache[bindingPath] = saved;
else delete require.cache[bindingPath];

function session() {
  const value = new VmSession();
  value.writeLocator = () => {};
  value.cleanup = () => {};
  return value;
}
function socket() { return { destroyed: false, writableLength: 0, writes: [], destroy() { this.destroyed = true; }, write(text) { this.writes.push(text); this.writableLength += Buffer.byteLength(text); return false; } }; }

test('stopped sessions release their process exit listeners', () => {
  const before = process.listenerCount('exit');
  for (let i = 0; i < 20; i++) { const s = session(); s.stop(); }
  assert.equal(process.listenerCount('exit'), before);
});

test('publish encodes one snapshot for all authenticated clients', () => {
  const s = session(); let snapshots = 0;
  s.server = { listening: true, close() {} };
  s.snapshot = () => { snapshots++; return { value: 42 }; };
  const clients = Array.from({length:16}, socket);
  clients.forEach(c => s.authenticatedClients.add(c));
  try { s.publish('test'); assert.equal(snapshots, 1); assert.ok(clients.every(c => c.writes.length === 1)); }
  finally { s.stop(); }
});

test('slow readers are disconnected before their write queue exceeds the frame budget', () => {
  const s = session(), client = socket();
  s.snapshot = () => ({ value: 'x'.repeat(64 * 1024) });
  try {
    for (let i = 0; i < 100; i++) s.send(client, 'snapshot');
    assert.equal(client.destroyed, true);
    assert.ok(client.writableLength <= 1024 * 1024);
  } finally { s.stop(); }
});

test('exit listener is restored when a stopped session is restarted', () => {
  const net = require('node:net');
  const { EventEmitter } = require('node:events');
  const original = net.createServer;
  const before = process.listenerCount('exit');
  const s = session();
  net.createServer = () => Object.assign(new EventEmitter(), {
    listen() {}, close() {},
  });
  try {
    for (let i = 0; i < 2; i++) {
      s.start();
      assert.equal(process.listenerCount('exit'), before + 1);
      s.stop();
      assert.equal(process.listenerCount('exit'), before);
    }
  } finally { s.stop(); net.createServer = original; }
});
