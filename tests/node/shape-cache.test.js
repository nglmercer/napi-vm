const {test} = require('node:test');
const assert = require('node:assert/strict');
const {Vm, AsyncSession} = require('../../index.js');

const setup = (x,y) => `
  var anchor={x:${x},y:${y}};
  function read(o){return o.x+o.y;}
  read(anchor);read(anchor);
  function churn(g){var s=0;for(var i=0;i<12000;i++){
    var o={x:i};o['key_'+g+'_'+i]=i;s+=o.x+o.x;
  }return s+read(anchor);}
  read(anchor);
`;
const sum = 12000*11999;

test('shape-cache pressure preserves live layouts across sync owner leases', () => {
  const first = new Vm(), second = new Vm();
  try {
    assert.equal(first.run(setup(13,29)), '42');
    assert.equal(second.run(setup(100,7)), '107');
    for (let generation=1; generation<=3; generation++) {
      assert.equal(first.run(`churn(${generation});`), String(sum+42));
      first.collectCycles();
      assert.equal(second.run('read(anchor);'), '107');
      assert.equal(second.run(`churn(${generation});`), String(sum+107));
      second.collectCycles();
      assert.equal(first.run('read(anchor);'), '42');
    }
    first.dispose();
    assert.equal(second.run('read(anchor);'), '107');
  } finally {first.dispose();second.dispose();}
});

test('async owner pressure preserves callbacks and another owner during cleanup', async () => {
  const session = new AsyncSession(), other = new Vm();
  try {
    assert.equal(await session.run(setup(13,29)), '42');
    assert.equal(other.run(setup(100,7)), '107');
    await session.exposeFunction('host', generation => Number(other.run(`churn(${generation});`)));
    assert.equal(await session.run('churn(1)+host(1);'), String(sum*2+42+107));
    await session.collectCycles();
    assert.equal(await session.run('read(anchor);'), '42');
    assert.equal(other.run('read(anchor);'), '107');
    session.dispose();
    assert.equal(other.run('churn(2);'), String(sum+107));
  } finally {session.dispose();other.dispose();}
});
