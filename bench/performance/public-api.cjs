'use strict';
// Persistent public API timings. Preparation and assertions are outside timers.
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
function measure(name, invoke, check, boundary, prepare=()=>{}) {
  for(let i=0;i<5;i++){prepare();check(invoke());}
  const times=[];
  for(let i=0;i<samples;i++) {
    let elapsed=0n;
    for(let j=0;j<iterations;j++) {
      prepare();const start=process.hrtime.bigint();const result=invoke();
      elapsed+=process.hrtime.bigint()-start;check(result);
    }
    times.push(Number(elapsed)/iterations);
  }
  const mid=median(times);
  results.push({name:'node_api/'+name,unit:'ns/call',backend:boundary,samples:times,
    summary:{median:mid,mad:median(times.map(x=>Math.abs(x-mid)))}});
}
function usingVm(run) {const vm=new Vm();try{run(vm);}finally{vm.dispose();}}
const eq=expected=>actual=>assert.deepEqual(actual,expected);
measure('construct_dispose',()=>{const vm=new Vm();vm.dispose();return true;},eq(true),'NAPI Vm construction and explicit disposal, including builtin setup/cleanup');
usingVm(vm=>{
  vm.run('var apiCounter=0;function answer(){return 42;}function sum8(a,b,c,d,e,f,g,h){return a+b+c+d+e+f+g+h;}');
  let expected=0;
  measure('run_cached_source',()=>vm.run('apiCounter++;apiCounter;'),r=>assert.equal(r,String(++expected)),'NAPI persistent run with warmed identical source and scalar string result');
  vm.setGlobal('scalar',42);
  measure('get_global_scalar',()=>vm.getGlobal('scalar'),eq('42'),'NAPI getGlobal with existing scalar and string result');
  measure('has_global',()=>vm.hasGlobal('scalar'),eq(true),'NAPI hasGlobal existing name');
  measure('set_global_scalar',()=>vm.setGlobal('scalar',42),()=>assert.equal(vm.getGlobal('scalar'),'42'),'NAPI setGlobal existing scalar; verification read excluded');
  measure('remove_global',()=>vm.removeGlobal('removable'),r=>{assert.equal(r,true);assert.equal(vm.hasGlobal('removable'),false);},'NAPI removeGlobal existing scalar; reseeding excluded',()=>vm.setGlobal('removable',42));
  measure('call_no_arguments',()=>vm.callFunction('answer',[]),eq(42),'NAPI named guest call, no arguments, scalar outbound result');
  const args=[1,2,3,4,5,6,7,8];
  measure('call_eight_numbers',()=>vm.callFunction('sum8',args),eq(36),'NAPI named guest call, eight scalar incoming arguments, scalar result');
  vm.exposeFunction('hostAdd',x=>x+1);
  vm.run('function callback(){return hostAdd(41);}function plain(){return 41+1;}');
  measure('call_plain_control',()=>vm.callFunction('plain',[]),eq(42),'NAPI named guest call with scalar guest addition');
  measure('call_host_callback',()=>vm.callFunction('callback',[]),eq(42),'NAPI named guest call, synchronous Node callback and scalar result');
  measure('replace_host_function',()=>vm.exposeFunction('hostAdd',x=>x+1),()=>assert.equal(vm.callFunction('callback',[]),42),'NAPI exposeFunction replacement including callback registration/revocation; invocation check excluded');
  vm.run('function inputSum(){var s=0;for(var k of Object.keys(inputObject))s+=inputObject[k];return s;}function outputObject(){return savedObject;}function inputEnds(){return inputBytes[0]+inputBytes[inputBytes.length-1];}function outputBytes(){return savedBytes;}');
  for(const size of [1,128]) {
    const input=Object.fromEntries(Array.from({length:size},(_,i)=>['key'+i,1]));
    measure('set_object_'+size,()=>vm.setGlobal('inputObject',input),()=>assert.equal(vm.callFunction('inputSum',[]),size),'NAPI setGlobal plain incoming object with '+size+' numeric own properties; guest sum check excluded');
    vm.run('var savedObject='+JSON.stringify(input)+';');
    measure('return_object_'+size,()=>vm.callFunction('outputObject',[]),eq(input),'NAPI named guest call and outgoing plain object with '+size+' numeric own properties');
  }
  for(const size of [1024,65536]) {
    const input=new Uint8Array(size);input[0]=123;input[size-1]=231;
    measure('set_bytes_'+size,()=>vm.setGlobal('inputBytes',input),()=>assert.equal(vm.callFunction('inputEnds',[]),354),'NAPI setGlobal full Uint8Array copy of '+size+' bytes; guest endpoint check excluded');
    vm.run(`var savedBytes=new Uint8Array(${size});savedBytes[0]=123;savedBytes[${size-1}]=231;`);
    measure('return_bytes_'+size,()=>vm.callFunction('outputBytes',[]),r=>{assert(r instanceof Uint8Array);assert.deepEqual(r,input);},'NAPI named guest call and outgoing Uint8Array copy of '+size+' bytes');
  }
  measure('validate_module',()=>vm.validateModule('export function answer(){return 42;}'),r=>assert.equal(r.valid,true),'NAPI validateModule valid small source; see implementation for parser/cache policy');
  measure('define_module',()=>vm.defineModule('apiModule','export const value=42;'),()=>assert(vm.hasModule('apiModule')),'NAPI deferred module source registration/replacement; import evaluation excluded');
  assert.equal(vm.run('import {value as deferredValue} from "apiModule";deferredValue;'),'42');
  measure('register_module',()=>vm.registerModule('apiEager','export const value=42;'),()=>assert(vm.hasModule('apiEager')),'NAPI eager module replacement, warmed identical source, export transaction and execution');
  assert.equal(vm.run('import {value as eagerValue} from "apiEager";eagerValue;'),'42');
  measure('has_module',()=>vm.hasModule('apiModule'),eq(true),'NAPI hasModule existing deferred module');
  measure('remove_module',()=>vm.removeModule('apiRemove'),r=>{assert(r);assert.equal(vm.hasModule('apiRemove'),false);},'NAPI removeModule deferred source; reseeding excluded',()=>vm.defineModule('apiRemove','export const value=1;'));
  const names=vm.listModules().sort();
  measure('list_modules',()=>vm.listModules(),r=>assert.deepEqual(r.sort(),names),'NAPI listModules existing small registry and outbound string array');
  measure('collect_cycles_idle',()=>vm.collectCycles(),r=>assert(Number.isInteger(r)&&r>=0),'NAPI explicit cycle collection on retained owner without deliberately created garbage');
});
console.log(JSON.stringify({schema:1,metadata:{bindingPath,
  addonSha256:createHash('sha256').update(readFileSync(bindingPath)).digest('hex'),
  scriptSha256:createHash('sha256').update(readFileSync(__filename)).digest('hex'),
  node:process.version,platform:process.platform,arch:process.arch,cpu:os.cpus()[0]?.model,samples,iterations,
  limitations:['Preparation and correctness checks excluded; timers surround one public API call except explicit construct/dispose pair.',
    'Checks may execute guest code between timed calls and warm caches or trigger GC.',
    'Persistent owner reused; no CPU isolation; allocation, automatic GC and host activity affect samples.',
    'Idle explicit GC is API overhead, not a garbage-collection throughput workload.',
    'No equivalent rquickjs host adapter or whole-application engine ranking.']},results},null,2));
