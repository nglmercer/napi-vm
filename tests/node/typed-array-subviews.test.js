const test = require('node:test');
const assert = require('node:assert/strict');
const { Vm } = require('../../index.js');

test('typed array and DataView exports copy only the visible nonzero-offset range', () => {
  const vm = new Vm();
  try {
    vm.run(`
      const backing = new ArrayBuffer(8 * 1024 * 1024);
      const bytes = new Uint8Array(backing, backing.byteLength - 16, 16);
      for (let i = 0; i < bytes.length; i++) bytes[i] = i + 1;
      function getBytes() { return bytes; }
      function getWords() { return new Uint16Array(backing, backing.byteLength - 16, 4); }
      function getData() { return new DataView(backing, backing.byteLength - 15, 5); }
      function getEmpty() { return new Uint8Array(backing, backing.byteLength, 0); }
      function getEmptyData() { return new DataView(backing, backing.byteLength, 0); }
    `);
    const bytes = vm.callFunction('getBytes', []);
    assert(bytes instanceof Uint8Array);
    assert.equal(bytes.byteOffset, 0);
    assert.equal(bytes.buffer.byteLength, 16);
    assert.deepEqual([...bytes], Array.from({ length: 16 }, (_, i) => i + 1));
    bytes[0] = 99;
    assert.equal(vm.run('bytes[0]'), '1', 'host mutation must not reach the guest');
    vm.run('bytes[1] = 88;');
    assert.equal(bytes[1], 2, 'guest mutation must not reach the exported copy');
    const words = vm.callFunction('getWords', []);
    assert(words instanceof Uint16Array);
    assert.equal(words.length, 4);
    assert.equal(words.buffer.byteLength, 8);
    assert.deepEqual([...new Uint8Array(words.buffer)], [1, 88, 3, 4, 5, 6, 7, 8]);
    const data = vm.callFunction('getData', []);
    assert(data instanceof DataView);
    assert.equal(data.byteOffset, 0);
    assert.equal(data.byteLength, 5);
    assert.equal(data.buffer.byteLength, 5);
    assert.deepEqual([...new Uint8Array(data.buffer)], [88, 3, 4, 5, 6]);
    assert.equal(vm.callFunction('getEmpty', []).buffer.byteLength, 0);
    assert.equal(vm.callFunction('getEmptyData', []).byteLength, 0);
  } finally { vm.dispose(); }
});

test('resized and detached backing stores preserve exported effective view bounds', () => {
  const vm = new Vm();
  try {
    vm.run(`
      const backing = new ArrayBuffer(32, { maxByteLength: 64 });
      const fixed = new Uint8Array(backing, 16, 8);
      const tracking = new Uint16Array(backing, 16);
      const data = new DataView(backing, 16, 8);
      function getFixed() { return fixed; }
      function getTracking() { return tracking; }
      function getData() { return data; }
    `);
    assert.equal(vm.callFunction('getFixed', []).byteLength, 8);
    assert.equal(vm.callFunction('getTracking', []).byteLength, 16);
    vm.run('backing.resize(20);');
    assert.equal(vm.callFunction('getFixed', []).byteLength, 0);
    assert.equal(vm.callFunction('getTracking', []).byteLength, 4);
    assert.equal(vm.callFunction('getData', []).byteLength, 0);
    vm.run('backing.transfer();');
    assert.equal(vm.callFunction('getFixed', []).byteLength, 0);
    assert.equal(vm.callFunction('getTracking', []).byteLength, 0);
    assert.equal(vm.callFunction('getData', []).byteLength, 0);
  } finally { vm.dispose(); }
});
