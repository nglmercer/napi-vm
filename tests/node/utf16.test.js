const test = require('node:test');
const assert = require('node:assert/strict');
const {Vm} = require('../../index.js');

test('structured N-API strings and property keys preserve UTF-16 units', () => {
  const vm = new Vm();
  const values = ['', '\0', '😀', '\ud800', '\udc00', '\udc00\ud800', '\ufdd0sD800'];
  vm.run('function identity(value) { return value; } function copy(value) { return {...value}; }');
  for (const value of values) {
    vm.setGlobal('input', value);
    assert.equal(vm.run('input.length'), String(value.length));
    assert.equal(vm.callFunction('identity', [value]), value);
    const object = {[value]: value};
    const result = vm.callFunction('copy', [object]);
    assert.deepEqual(Reflect.ownKeys(result), [value]);
    assert.equal(result[value], value);
  }
});

test('N-API host callbacks preserve surrogate arguments and results', () => {
  const vm = new Vm();
  vm.exposeFunction('echo', value => value);
  assert.equal(vm.run("echo('\\ud800').charCodeAt(0)"), '55296');
});

test('global names and asynchronous values preserve surrogate code units', async () => {
  const {AsyncSession} = require('../../index.js');
  const vm = new Vm();
  const key = '\ud800';
  vm.setGlobal(key, 42);
  assert.equal(vm.getGlobal(key), '42');
  assert.equal(vm.run("globalThis['\\ud800']"), '42');
  assert.equal(vm.hasGlobal(key), true);
  vm.run("globalThis['\\udc00']=function(value){return value;};");
  assert.equal(vm.callFunction('\udc00', ['\ud800']), '\ud800');
  const session = new AsyncSession();
  try {
    await session.setGlobal(key, '\udc00');
    assert.equal(await session.run("globalThis['\\ud800'].charCodeAt(0)"), '56320');
  } finally {session.dispose();vm.dispose();}
});

test('asynchronous host rejection messages preserve surrogate units', async () => {
  const {AsyncSession} = require('../../index.js');
  const session = new AsyncSession();
  try {
    await session.exposeFunction('rejectUnits', async () => {throw new Error('\ud800');}, true);
    assert.equal(await session.run("try {await rejectUnits();} catch(e) {e.message.charCodeAt(0);}"), '55296');
  } finally {session.dispose();}
});

test('script source inputs preserve raw surrogate code units', async () => {
  const {AsyncSession, runCode} = require('../../index.js');
  const source = "'" + '\ud800' + "'.charCodeAt(0)";
  const vm = new Vm();
  const session = new AsyncSession();
  try {
    assert.equal(vm.run(source), '55296');
    assert.equal(vm.validateModule(source).valid, true);
    assert.throws(() => vm.defineModule('bad', source), /unpaired surrogates/);
    vm.exposeFunction('\udc00', value => value);
    assert.equal(vm.callFunction('\udc00', ['\ud800']), '\ud800');
    assert.equal(vm.removeGlobal('\udc00'), true);
    assert.equal(runCode(source), '55296');
    assert.equal(await vm.runAsync(source), '55296');
    assert.equal(await session.run(source), '55296');
  } finally {session.dispose();vm.dispose();}
});
