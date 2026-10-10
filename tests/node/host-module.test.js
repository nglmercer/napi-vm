const test = require('node:test');
const assert = require('node:assert/strict');
const {Vm} = require('../../index.js');

test('host module exports participate in linking and reject revoked exports', () => {
  const vm = new Vm();
  try {
    const globals = vm.registerHostModule('host', {answer: () => 42, removed: () => 1});
    vm.registerModule('app', "import {answer} from 'host'; export function read(){return answer();}");
    assert.equal(vm.run("import {read} from 'app'; read();"), '42');
    assert.throws(() => vm.registerHostModule('host', {answer: 1}), /must be a function/);
    assert.equal(vm.run(`${globals.find(name => name.endsWith('_answer'))}();`), '42');
    vm.registerHostModule('host', {answer: () => 43});
    assert.equal(vm.hasGlobal(globals.find(name => name.endsWith('_removed'))), false);
    vm.registerModule('updated', "import {answer} from 'host'; export function read(){return answer();}");
    assert.equal(vm.run("import {read as updatedRead} from 'updated'; updatedRead();"), '43');
    assert.throws(() => vm.registerModule('bad', "import {removed} from 'host'; removed();"), /SyntaxError/);
  } finally {
    vm.dispose();
  }
});

test('async host exports work through a registered module', async () => {
  const vm = new Vm();
  try {
    vm.registerHostModule('host', {answer: async () => 42}, {async: ['answer']});
    vm.registerModule('app', "import {answer} from 'host'; export async function read(){return await answer();}");
    assert.equal(await vm.runAsync("import {read} from 'app'; await read();"), '42');
  } finally {
    vm.dispose();
  }
});

test('a rejected bridge binding rolls back replaced functions and linked source', () => {
  const vm = new Vm();
  try {
    const [global] = vm.registerHostModule('host', {answer: () => 42});
    vm.run('Object.preventExtensions(globalThis);');
    assert.throws(() => vm.registerHostModule('host', {answer: () => 43, extra: () => 1}), /Cannot install host module binding/);
    assert.equal(vm.run(`${global}();`), '42');
    assert.equal(vm.hasGlobal('__hostmod_host_extra'), false);
    vm.registerModule('app', "import {answer} from 'host'; export function read(){return answer();}");
    assert.equal(vm.run("import {read} from 'app'; read();"), '42');
  } finally {
    vm.dispose();
  }
});
