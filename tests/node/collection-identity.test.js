const {test}=require('node:test');
const assert=require('node:assert/strict');
const {Vm}=require('../../index.js');

function guest(source,check) {
  const vm=new Vm();
  try {vm.run(`function result(){${source}}`);check(vm.callFunction('result',[]));}
  finally {vm.dispose();}
}

test('outbound Map and Set aliases preserve native identity and insertion order',()=>{
  guest("var m=new Map([['b',2],['a',1]]);var s=new Set(['b','a']);return [m,m,s,s];",r=>{
    assert(r[0] instanceof Map);assert(r[2] instanceof Set);
    assert.equal(r[0],r[1]);assert.equal(r[2],r[3]);
    assert.deepEqual([...r[0]],[['b',2],['a',1]]);assert.deepEqual([...r[2]],['b','a']);
  });
});
test('self-references in Map keys, Map values and Sets survive conversion',()=>{
  guest("var m=new Map();m.set(m,m);var s=new Set();s.add(s);return [m,s];",r=>{
    assert.equal(r[0].get(r[0]),r[0]);assert(r[1].has(r[1]));
  });
});
test('mixed collection array and object cycles preserve shared keys',()=>{
  guest("var key={id:7};var m=new Map();var s=new Set();var o={key:key,map:m,set:s};var a=[o,m,s];m.set(key,a);m.set('root',o);s.add(m);s.add(o);return o;",r=>{
    const a=r.map.get(r.key);assert.equal(a[0],r);assert.equal(a[1],r.map);assert.equal(a[2],r.set);
    assert.equal(r.map.get('root'),r);assert(r.set.has(r.map));assert(r.set.has(r));
  });
});
test('distinct equal collections are not merged',()=>{
  guest("return [new Map([[1,2]]),new Map([[1,2]]),new Set([1]),new Set([1])];",r=>{
    assert.notEqual(r[0],r[1]);assert.notEqual(r[2],r[3]);
    assert.deepEqual([...r[0]],[...r[1]]);assert.deepEqual([...r[2]],[...r[3]]);
  });
});
test('large alias graphs do not expand duplicate collections',()=>{
  guest("var m=new Map();for(var i=0;i<256;i++)m.set(i,i);var a=[];for(var i=0;i<512;i++)a.push(m);return a;",r=>{
    assert.equal(r.length,512);assert(r.every(x=>x===r[0]));assert.equal(r[0].size,256);
  });
});
test('depth limit still rejects deep noncyclic collection graphs',()=>{
  const vm=new Vm();
  try {
    vm.run("function result(){var m=new Map();for(var i=0;i<600;i++)m=new Map([['next',m]]);return m;}");
    assert.throws(()=>vm.callFunction('result',[]),/too deep/);
  } finally {vm.dispose();}
});
