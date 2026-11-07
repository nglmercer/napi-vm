'use strict';
// Node context APIs with the canonical fixture export; no native/WASM adapters.
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const os=require('node:os');
const {createHash}=require('node:crypto');
const sha=b=>createHash('sha256').update(b).digest('hex');
const bytes=fs.readFileSync(process.env.INTERNATIONAL_FIXTURES);
const fixtures=JSON.parse(bytes);
assert.equal(fixtures.kind,'international_workloads');
const samples=Number(process.env.INTERNATIONAL_SAMPLES??15),iterations=Number(process.env.INTERNATIONAL_ITERATIONS??10);
assert(Number.isSafeInteger(samples)&&samples>0&&samples%2===1);assert(Number.isSafeInteger(iterations)&&iterations>0);assert(global.gc);
const median=a=>[...a].sort((a,b)=>a-b)[Math.floor(a.length/2)];
const capabilities=JSON.parse(vm.runInNewContext(fixtures.probe_source));
const results=[];let nonce=0;
for(const w of fixtures.workloads) {
  assert.equal(w.source_sha256,sha(w.source));
  const missing=w.requires.filter(name=>!capabilities[name]);
  let problem;
  if(!missing.length) {
    try {const value=vm.runInNewContext(w.source);if(value!==w.expected)problem='expected '+w.expected+', received '+String(value);}
    catch(error){problem=String(error.message??error);}
  }
  let owner,fn;
  for(const mode of ['fresh_source','warm_source','warm_call']) {
    const base={engine:'node-v8-context',mode,workload:w.name,source_sha256:w.source_sha256,unit:'ns/call',status:missing.length?'unsupported':problem?'incompatible_result':'measured',missing};
    if(missing.length||problem){results.push({...base,...(problem?{diagnostic:problem}:{})});continue;}
    if(mode==='warm_source') {
      owner=vm.createContext({});vm.runInContext(w.setup,owner);fn=vm.runInContext('bench;',owner);
    }
    const invoke=()=>mode==='fresh_source'?vm.runInNewContext(w.source+'\n// fresh '+(++nonce)):mode==='warm_source'?vm.runInContext(w.invocation,owner):fn();
    for(let i=0;i<5;i++)assert.equal(invoke(),w.expected);
    global.gc();
    const times=[];
    for(let sample=0;sample<samples;sample++) {
      let sum=0;const start=process.hrtime.bigint();
      for(let i=0;i<iterations;i++)sum+=invoke();
      const elapsed=Number(process.hrtime.bigint()-start)/iterations;
      assert.equal(sum,w.expected*iterations);times.push(elapsed);global.gc();
    }
    const mid=median(times);results.push({...base,samples:times,median:mid,mad:median(times.map(x=>Math.abs(x-mid)))});
  }
  owner=undefined;fn=undefined;global.gc();
}
console.log(JSON.stringify({schema:1,kind:'international_child',samples,iterations,warmup_calls:5,capabilities:{'node-v8-context':capabilities},results,metadata:{node:process.version,v8:process.versions.v8,icu:process.versions.icu??'unavailable',platform:process.platform,arch:process.arch,cpu:os.cpus()[0]?.model??'unknown',fixtures_sha256:sha(bytes),script_sha256:sha(fs.readFileSync(__filename)),host_gc:true}}));
