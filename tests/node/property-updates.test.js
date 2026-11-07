const test = require('node:test');
const assert = require('node:assert/strict');
const { runInNewContext } = require('node:vm');
const { Vm } = require('../../index.js');
const cases = require('../fixtures/property-update-cases.cjs');

for (const { name, source, expected } of cases) {
  test(name, () => {
    assert.equal(String(runInNewContext(source)), expected, 'Node oracle');
    const vm = new Vm();
    try { assert.equal(vm.run(source), expected); }
    finally { vm.dispose(); }
  });
}
