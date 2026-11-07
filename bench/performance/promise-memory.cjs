'use strict';
// One native owner, pending promise fan-out. Tracked cells and process RSS
// measure different things; snapshots are not allocation totals or latency.
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const os=require('node:os');
const {createHash}=require('node:crypto');
assert(process.env.NAPI_BENCH_BINDING,'Set NAPI_BENCH_BINDING');
const binding=path.resolve(process.env.NAPI_BENCH_BINDING);
const count=Number(process.argv[2]??512);
const species=process.argv[3]??'native';
assert(['native','custom'].includes(species));
assert(Number.isSafeInteger(count)&&count>0&&count<=100000);
const sha=b=>createHash('sha256').update(b).digest('hex');
const phases=[];
function snapshot(phase,vm) {
  if(global.gc)global.gc();
  const status=fs.readFileSync('/proc/self/status','utf8');
  const bytes=k=>Number(status.match(new RegExp('^'+k+':\\s+(\\d+)','m'))[1])*1024;
  phases.push({phase,rss_bytes:bytes('VmRSS'),high_water_bytes:bytes('VmHWM'),engine_stats:vm?JSON.parse(vm.heapStats()):null});
}
const setup='var source=null,release=null,items=null,total=0,completed=0;function Species(executor){executor(function(v){total+=v;completed++;},function(e){throw e;});}function populate(){source=new Promise(r=>{release=r;});'+
  (species==='custom'?'source.constructor={[Symbol.species]:Species};':'')+
  'items=[];for(var i=0;i<'+count+';i++)items.push(source.then(v=>v+1));return items.length;}';
snapshot('baseline');
const {Vm}=require(binding);const vm=new Vm();
try {
  vm.run(setup);vm.collectCycles();snapshot('ready',vm);
  assert.equal(vm.callFunction('populate',[]),count);
  snapshot('pending',vm);vm.collectCycles();snapshot('pending_collected',vm);
  vm.run(species==='native'?'release(41);Promise.all(items).then(a=>{total=a.reduce((s,x)=>s+x,0);completed=a.length;});':'release(41);');
  assert.equal(vm.run('total;'),String(count*42));
  assert.equal(vm.run('completed;'),String(count));
  vm.collectCycles();snapshot('settled',vm);
  vm.run('source=null;release=null;items=null;');vm.collectCycles();snapshot('released',vm);
} finally {vm.dispose();}
snapshot('disposed');
console.log(JSON.stringify({schema:1,kind:'native_pending_promises',node:process.version,platform:process.platform,arch:process.arch,cpu:os.cpus()[0]?.model,species,count,setup,source_sha256:sha(setup),addon_sha256:sha(fs.readFileSync(binding)),script_sha256:sha(fs.readFileSync(__filename)),host_gc:typeof global.gc==='function',phases,limitations:['Tracked cells are owner-local registry counts; pending_collected follows explicit guest GC. These are not allocated bytes.','RSS includes Node, native code, builtins and allocator caches; high-water marks need not fall after release.','Result checking and additional Promise.all registration occur after the pending snapshots.']}));
