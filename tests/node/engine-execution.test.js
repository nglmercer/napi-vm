const test = require('node:test');
const assert = require('node:assert/strict');
const { Vm } = require('../../index.js');

test('pending module dependencies evaluate once and resume after release', () => {
  const vm = new Vm();
  vm.run('var release; var gate = new Promise(r => release = r); var hits = 0; var answer = 0;');
  vm.defineModule('dependency', 'await gate; hits++; export const value = 42;');
  vm.defineModule('entry', 'import { value } from "dependency"; export default value;');
  vm.run('import("entry").then(m => answer = m.default); import("entry");');
  assert.equal(vm.run('hits;'), '0');
  vm.run('release();');
  assert.equal(vm.run('answer + ":" + hits;'), '42:1');
  vm.dispose();
});

test('link errors reject imports before module side effects', () => {
  const vm = new Vm();
  vm.run('var hits = 0; var failure;');
  vm.defineModule('source', 'hits++; export const value = 1;');
  vm.defineModule('bad', 'import { missing } from "source"; hits++;');
  vm.run('import("bad").catch(e => failure = e.name);');
  assert.equal(vm.run('failure + ":" + hits;'), 'SyntaxError:0');
  vm.dispose();
});

test('weak references permit collection and finalization runs at a job checkpoint', () => {
  const vm = new Vm();
  vm.run('var finalized = ""; var registry = new FinalizationRegistry(v => finalized = v); var target = {}; var ref = new WeakRef(target); registry.register(target, "done"); target = null;');
  vm.collectCycles();
  assert.equal(vm.run('ref.deref() === undefined;'), 'true');
  assert.equal(vm.run('finalized;'), 'done');
  vm.dispose();
});
