'use strict';
// Public APIs, one process. Native/builtin loading and WASM compilation are
// reported separately; source preparation and assertions are outside timers.
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const os=require('node:os');
const {pathToFileURL}=require('node:url');
const {createHash}=require('node:crypto');
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const median=values=>[...values].sort((a,b)=>a-b)[Math.floor(values.length/2)];
const samples=Number(process.env.PLATFORM_BENCH_SAMPLES??15);
const iterations=Number(process.env.PLATFORM_BENCH_ITERATIONS??10);
assert(Number.isSafeInteger(samples)&&samples>0&&samples%2===1);
assert(Number.isSafeInteger(iterations)&&iterations>0);
const binding=path.resolve(process.env.NAPI_BENCH_BINDING);
const wasmPackage=path.resolve(process.env.NAPI_VM_WASM_PACKAGE);
const workloadPath=path.resolve(process.env.PLATFORM_BENCH_WORKLOADS);
const fixtureBytes=fs.readFileSync(workloadPath);
const fixture=JSON.parse(fixtureBytes);
assert.equal(fixture.schema,1);assert.equal(fixture.kind,'guest_workloads');assert(fixture.workloads.length>0);
const names=new Set();
for(const w of fixture.workloads) {
  assert(!names.has(w.name));names.add(w.name);
  assert.equal(w.source,w.setup+'\n'+w.invocation);
  assert.equal(w.source_sha256,sha(w.source));
  assert(Number.isFinite(w.expected));
}
const wasmPath=path.join(wasmPackage,'napi_vm_bg.wasm');
const gluePath=path.join(wasmPackage,'napi_vm.js');
const wasmBytes=fs.readFileSync(wasmPath);
function packageFiles(directory,prefix='') {
  const result={};
  for(const entry of fs.readdirSync(directory,{withFileTypes:true}).sort((a,b)=>a.name<b.name?-1:a.name>b.name?1:0)) {
    const relative=prefix+entry.name;
    if(entry.isDirectory())Object.assign(result,packageFiles(path.join(directory,entry.name),relative+'/'));
    else {assert(entry.isFile(),'unexpected non-file package artifact');result[relative]=sha(fs.readFileSync(path.join(directory,entry.name)));}
  }
  return result;
}
const packageHashes=packageFiles(wasmPackage);
const sortedPackageHashes=Object.fromEntries(Object.entries(packageHashes).sort(([a],[b])=>a<b?-1:a>b?1:0));
const hashes={addon:sha(fs.readFileSync(binding)),wasm:sha(wasmBytes),glue:sha(fs.readFileSync(gluePath)),package:sha(JSON.stringify(sortedPackageHashes)),workloads:sha(fixtureBytes),script:sha(fs.readFileSync(__filename))};
const start=()=>process.hrtime.bigint();
const since=t=>Number(start()-t);

async function main() {
  if(global.gc)global.gc();
  const startup={};
  let t=start();const {Vm}=require(binding);startup.native_addon_load_ns=since(t);
  t=start();const nodeVm=require('node:vm');startup.node_vm_builtin_load_ns=since(t);
  t=start();const wasm=await import(pathToFileURL(gluePath).href);startup.wasm_glue_import_ns=since(t);
  t=start();const compiled=await WebAssembly.compile(wasmBytes);startup.wasm_compile_ns=since(t);
  t=start();const exports=wasm.initSync({module:compiled});startup.wasm_instantiate_init_ns=since(t);
  const linearReady=exports.memory.buffer.byteLength;
  const adapters=[
    {name:'napi-native',make:()=>new Vm(),run:(vm,source)=>vm.run(source),dispose:vm=>vm.dispose(),
      check:(raw,w)=>assert.equal(raw,String(w.expected))},
    {name:'napi-wasm',make:()=>new wasm.WasmVm(),run:(vm,source)=>vm.run(source),dispose:vm=>vm.free(),
      check:(raw,w)=>{assert(raw.ok,raw.error);assert.equal(raw.value,String(w.expected));assert.deepEqual(raw.logs,[]);}},
    {name:'node-v8',make:()=>nodeVm.createContext({}),run:(vm,source)=>nodeVm.runInContext(source,vm),dispose:()=>{},
      check:(raw,w)=>assert.equal(raw,w.expected)}
  ];
  const results=[];
  function row(engine,mode,w,timings) {
    const mid=median(timings);
    results.push({engine,mode,workload:w.name,source_sha256:w.source_sha256??null,unit:mode==='construct_dispose'?'ns/instance':'ns/call',
      samples:timings,median:mid,mad:median(timings.map(n=>Math.abs(n-mid)))});
  }
  const lifecycle=adapters.map(()=>[]);
  for(const a of adapters)for(let i=0;i<5;i++){const vm=a.make();a.dispose(vm);}
  if(global.gc)global.gc();
  for(let sample=0;sample<samples;sample++) {
    for(let slot=0;slot<adapters.length;slot++) {
      const index=(sample+slot)%adapters.length;const a=adapters[index];let elapsed=0n;
      for(let i=0;i<iterations;i++){const t=start();const vm=a.make();a.dispose(vm);elapsed+=start()-t;}
      lifecycle[index].push(Number(elapsed)/iterations);
    }
    if(global.gc)global.gc();
  }
  adapters.forEach((a,i)=>row(a.name,'construct_dispose',{name:'vm_lifecycle'},lifecycle[i]));
  let nonce=0;
  for(const w of fixture.workloads) {
    for(const mode of ['fresh_source','warm_source']) {
      const owners=mode==='warm_source'?adapters.map(a=>a.make()):[];
      try {
        if(owners.length)adapters.forEach((a,i)=>{
          const raw=a.run(owners[i],w.setup);if(a.name==='napi-wasm')assert(raw.ok,raw.error);
        });
        const invoke=(index,source)=>{
          const a=adapters[index];
          if(owners.length)return a.run(owners[index],source);
          const vm=a.make();try{return a.run(vm,source);}finally{a.dispose(vm);}
        };
        for(let index=0;index<adapters.length;index++)
          for(let i=0;i<5;i++)adapters[index].check(invoke(index,owners.length?w.invocation:w.source+'\n// public fresh '+(++nonce)),w);
        const times=adapters.map(()=>[]);
        for(let sample=0;sample<samples;sample++) {
          for(let slot=0;slot<adapters.length;slot++) {
            const index=(sample+slot)%adapters.length;let elapsed=0n;
            for(let i=0;i<iterations;i++) {
              const source=owners.length?w.invocation:w.source+'\n// public fresh '+(++nonce);
              const t=start();const raw=invoke(index,source);elapsed+=start()-t;
              adapters[index].check(raw,w);
            }
            times[index].push(Number(elapsed)/iterations);
          }
          if(global.gc)global.gc();
        }
        adapters.forEach((a,i)=>row(a.name,mode,w,times[i]));
      } finally {owners.forEach((vm,i)=>adapters[i].dispose(vm));}
      if(global.gc)global.gc();
    }
  }
  console.log(JSON.stringify({schema:1,kind:'public_platform_timing',samples,iterations,warmup_calls:5,
    metadata:{binding,wasm_package:wasmPackage,workload_path:workloadPath,hashes,node:process.version,v8:process.versions.v8,
      platform:process.platform,arch:process.arch,cpu:os.cpus()[0]?.model,host_gc:typeof global.gc==='function',
      wasm_bytes:wasmBytes.length,wasm_linear_ready_bytes:linearReady,wasm_linear_final_bytes:exports.memory.buffer.byteLength,
      limitations:['Public source APIs, not bare engines or retained guest-function calls.',
        'Native returns a string; WASM returns an ok/value/error/logs object; Node returns a number. API serialization stays timed; assertions and fresh-comment preparation do not.',
        'Fresh includes creation/evaluation/disposal. Node context references are released without synchronous guest-heap disposal; V8 host GC runs between batches.',
        'Native/WASM automatic guest cycle collection remains in their public APIs. No extra native-only explicit guest GC is imposed.',
        'WASM compile/init and module loads are one first-use observation per fresh process, after hashing/reading files; process startup and disk-cold I/O are excluded.',
        'Five warmup calls do not guarantee identical JIT/tiering states. Backend order rotates per sample; no CPU or host isolation.',
        'Native/WASM builtin/console setup and bridge result formats differ. No aggregate runtime score is computed.']},
    startup,workloads:fixture.workloads,results},null,2));
}
main().catch(error=>{console.error(error);process.exitCode=1;});
