const assert = require('node:assert/strict');
const test = require('node:test');
const {Vm} = require('../../index.js');

test('NAPI VMs expose engine globals by default', () => {
  const vm = new Vm();
  for (const name of ['console', 'setTimeout', 'queueMicrotask', 'TextEncoder', 'Buffer', 'process', 'fetch', 'WebSocket']) {
    assert.equal(vm.run(`typeof ${name} === 'undefined'`), 'true', name);
  }
  vm.run('var answer = 0; Promise.resolve(42).then(value => answer = value);');
  assert.equal(vm.run('answer'), '42');
});

test('runtime installation is explicit and requires compiled features', () => {
  const vm = new Vm();
  try {
    vm.enableRuntime({timers: true});
  } catch (error) {
    assert.match(error.message, /runtime Cargo feature/);
    assert.equal(vm.run("typeof setTimeout === 'undefined'"), 'true');
    return;
  }
  assert.equal(vm.run("typeof setTimeout === 'function'"), 'true');
  assert.equal(vm.run('var seen = 0; setTimeout(() => seen++, 1); seen;'), '0');
  assert.equal(vm.run('seen;'), '1');
  assert.equal(vm.run("typeof fetch === 'undefined'"), 'true');
});
