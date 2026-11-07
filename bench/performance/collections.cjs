'use strict';
// Select a verified addon explicitly. Setup and correctness checks are untimed.
const assert = require('node:assert/strict');
const {createHash} = require('node:crypto');
const {readFileSync} = require('node:fs');
const path = require('node:path');
const os = require('node:os');
assert(process.env.NAPI_BENCH_BINDING, 'Set NAPI_BENCH_BINDING');
const bindingPath = path.resolve(process.env.NAPI_BENCH_BINDING);
const {Vm} = require(bindingPath);
const samples = Number(process.env.NAPI_BENCH_SAMPLES ?? 15);
const iterations = Number(process.env.NAPI_BENCH_ITERATIONS ?? 10);
assert(Number.isSafeInteger(samples) && samples > 0 && samples % 2 === 1);
assert(Number.isSafeInteger(iterations) && iterations > 0);
const median = values => [...values].sort((a,b)=>a-b)[Math.floor(values.length/2)];
const results = [];
for (const kind of ['Map','Set']) {
  for (const references of [1,64]) {
    const vm = new Vm();
    try {
      const source = `var collection=new ${kind}();for(var i=0;i<256;i++)collection.${kind==='Map'?'set(i,i)':'add(i)'};var output=[];for(var i=0;i<${references};i++)output.push(collection);function get(){return output;}`;
      vm.run(source);
      let identity;
      const check = value => {
        assert(Array.isArray(value)); assert.equal(value.length,references);
        for (const entry of value) {
          assert(entry instanceof (kind==='Map'?Map:Set)); assert.equal(entry.size,256);
          assert(entry.has(0) && entry.has(255));
          if (kind==='Map') assert.equal(entry.get(255),255);
        }
        const observed = value.every(entry=>entry===value[0]);
        if (identity === undefined) identity = observed;
        assert.equal(observed,identity,'identity behavior changed within run');
      };
      for(let i=0;i<5;i++)check(vm.callFunction('get',[]));
      const timings=[];
      for(let i=0;i<samples;i++) {
        let elapsed=0n;
        for(let j=0;j<iterations;j++) {
          const start=process.hrtime.bigint();const result=vm.callFunction('get',[]);
          elapsed+=process.hrtime.bigint()-start;check(result);
        }
        timings.push(Number(elapsed)/iterations);
      }
      const mid=median(timings);
      results.push({name:`${kind.toLowerCase()}_${references}_references`,unit:'ns/call',source,
        expectedEntries:256,references,sharedIdentityPreserved:identity,samples:timings,
        summary:{median:mid,mad:median(timings.map(x=>Math.abs(x-mid)))}});
    } finally {vm.dispose();}
  }
}
console.log(JSON.stringify({schema:1,metadata:{bindingPath,
  addonSha256:createHash('sha256').update(readFileSync(bindingPath)).digest('hex'),
  scriptSha256:createHash('sha256').update(readFileSync(__filename)).digest('hex'),
  node:process.version,platform:process.platform,arch:process.arch,cpu:os.cpus()[0]?.model,
  samples,iterations,limitations:['Guest setup and assertions excluded; native guest call and outbound conversion included.',
    'Alias identity is recorded separately so the known before-build defect remains visible.',
    'No CPU isolation; GC and host activity can affect batches.']},results},null,2));
