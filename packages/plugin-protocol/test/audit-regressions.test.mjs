import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { RpcPeer, encodeFrame, PluginError } from '../dist/index.js';

class Socket extends EventEmitter {
  destroyed = false;
  frames = [];
  setNoDelay() {}
  write(frame, callback) { this.frames.push(frame); callback?.(); return true; }
  destroy() { this.destroyed = true; }
}

test('negotiated depth applies to subsequent frames in the same TCP chunk', () => {
  const socket = new Socket();
  const peer = new RpcPeer(socket, { role: 'h', sessionId: 'audit' });
  let events = 0;
  peer.on('event', () => { events++; peer.negotiate({ maxDepth: 3 }); });
  socket.emit('data', Buffer.concat([
    encodeFrame({ jsonrpc: '2.0', method: 'system.event', params: null }),
    encodeFrame({ jsonrpc: '2.0', method: 'system.event', params: { a: { b: { c: 1 } } } }),
  ]));
  assert.equal(events, 1);
  assert.equal(peer.isClosed, true);
});

test('negotiated frame size applies within the same TCP chunk', () => {
  const socket = new Socket();
  const peer = new RpcPeer(socket, { role: 'h', sessionId: 'audit' });
  let events = 0;
  peer.on('event', () => { events++; peer.negotiate({ maxFrameBytes: 128 }); });
  socket.emit('data', Buffer.concat([
    encodeFrame({ jsonrpc: '2.0', method: 'system.event', params: null }),
    encodeFrame({ jsonrpc: '2.0', method: 'system.event', params: 'x'.repeat(256) }),
  ]));
  assert.equal(events, 1);
  assert.equal(peer.isClosed, true);
});

test('error message truncation preserves Unicode and the 2048 character wire limit', () => {
  const wire = new PluginError('INTERNAL_ERROR', 'a'.repeat(2047) + '🙂' + 'z').toWire();
  assert.doesNotThrow(() => encodeFrame(wire));
  assert.equal([...wire.message].length, 2048);
  assert.ok(wire.message.endsWith('🙂'));
});

test('receivers accept 2048 astral error characters and keep the peer usable', async () => {
  const socket = new Socket();
  const peer = new RpcPeer(socket, { role:'h', sessionId:'audit' });
  const pending = peer.request('system.ping', {});
  const rejection = assert.rejects(pending, e => e.code === 'INTERNAL_ERROR' && [...e.message].length === 2048);
  socket.emit('data', encodeFrame({jsonrpc:'2.0',id:'h:audit:1',error:{code:-32603,message:'🙂'.repeat(2048),data:{code:'INTERNAL_ERROR'}}}));
  await rejection;
  assert.equal(peer.isClosed, false);
  peer.close();
});
