'use strict';
// One native owner per process. RSS includes Node, native arenas and builtins.
const assert=require('node:assert/strict');
const {createHash}=require('node:crypto');
const {readFileSync}=require('node:fs');
const path=require('node:path');
assert(process.env.NAPI_BENCH_BINDING,'Set NAPI_BENCH_BINDING');
const bindingPath=path.resolve(process.env.NAPI_BENCH_BINDING);
const kind=process.argv[2];
const collections=Number(process.argv[3]);
const entries=Number(process.argv[4]);
const rounds=3;
assert(['Map','Set'].includes(kind));
assert(Number.isSafeInteger(collections)&&collections>0);
assert(Number.isSafeInteger(entries)&&entries>0);
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const snapshots=[];
function snapshot(phase,round,vm) {
  const status=readFileSync('/proc/self/status','utf8');
  const bytes=field=>Number(status.match(new RegExp('^'+field+':\\s+(\\d+)','m'))[1])*1024;
  snapshots.push({phase,round,rss_bytes:bytes('VmRSS'),high_water_bytes:bytes('VmHWM'),
    engine_stats:vm?JSON.parse(vm.heapStats()):null});
}
snapshot('baseline',0);
const {Vm}=require(bindingPath);
const vm=new Vm();
const source='var retained=null;function populate(){retained=[];for(var j=0;j<'+collections+
  ';j++){var m=new '+kind+'();for(var i=0;i<'+entries+';i++)m.'+(kind==='Map'?'set(i,i)':'add(i)')+
  ';retained.push(m);}return retained.length;}function verify(){var n=0;for(var j=0;j<retained.length;j++){var m=retained[j];if(m.size!=='+entries+
  '||!m.has(0)||!m.has('+(entries-1)+'))throw Error("collection content mismatch");n+=m.size;}return n;}';
try {
  vm.run(source);vm.collectCycles();snapshot('ready',0,vm);
  for(let round=1;round<=rounds;round++) {
    assert.equal(vm.callFunction('populate',[]),collections);
    assert.equal(vm.callFunction('verify',[]),collections*entries);
    snapshot('allocated',round,vm);vm.collectCycles();snapshot('collected',round,vm);
  }
  vm.run('retained=null;');vm.collectCycles();snapshot('released',rounds,vm);
} finally {vm.dispose();}
snapshot('disposed',rounds);
console.log(JSON.stringify({schema:1,kind,collections,entries,rounds,source_sha256:sha(source),
  addon_sha256:sha(readFileSync(bindingPath)),script_sha256:sha(readFileSync(__filename)),snapshots}));
