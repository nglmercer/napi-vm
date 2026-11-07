import hashlib,json,os,statistics,subprocess
from pathlib import Path
root=Path.cwd();p=root/'artifacts/performance';out=p/'async-promise-memory.json'
assert not out.exists()
script=root/'bench/performance/promise-memory.cjs'
sha=lambda q:hashlib.sha256(q.read_bytes()).hexdigest()
builds={'before':p/'platform-final-after-current-build','candidate':p/'full-async-suite/platform-build'}
records={k:json.loads((v/'build.json').read_text()) for k,v in builds.items()}
assert all(records['before'][k]==records['candidate'][k] for k in ['profiles','rustc','wasm_pack','build_environment','helpers'])
changes=[k for k,h in records['before']['inputs'].items() if records['candidate']['inputs'][k]!=h]
assert sorted(changes)==['crates/napi-vm-core/src/builtins/promise.rs','crates/napi-vm-core/src/interpreter/promise.rs']
runs=[]
for count in [32,512,4096]:
 for species in ['native','custom']:
  for sample in range(3):
   for label in (['before','candidate'] if sample%2==0 else ['candidate','before']):
    directory=builds[label];addon=directory/'napi-vm.node'
    child=json.loads(subprocess.check_output(['node','--expose-gc',str(script),str(count),species],env={**os.environ,'NAPI_BENCH_BINDING':str(addon)},text=True))
    assert child['species']==species and child['count']==count and child['host_gc'] is True
    assert child['addon_sha256']==records[label]['artifacts']['napi-vm.node']==sha(addon)
    assert child['script_sha256']==sha(script)
    assert child['source_sha256']==hashlib.sha256(child['setup'].encode()).hexdigest()
    phases={x['phase']:x for x in child['phases']};assert len(phases)==len(child['phases'])==7
    assert set(phases)==set(['baseline','ready','pending','pending_collected','settled','released','disposed'])
    for phase in phases.values():
     assert type(phase['rss_bytes']) is int and phase['rss_bytes']>0
     if phase['engine_stats'] is not None:assert type(phase['engine_stats']['tracked']) is int and phase['engine_stats']['tracked']>=0
    runs.append({'build':label,'sample':sample,'report':child})
    print(label,count,species,sample,flush=True)
results=[]
for count in [32,512,4096]:
 for species in ['native','custom']:
  result={'count':count,'species':species,'builds':{}}
  hashes=set();hosts=set()
  for label in builds:
   samples=[x['report'] for x in runs if x['build']==label and x['report']['count']==count and x['report']['species']==species]
   assert len(samples)==3
   values={'pending_tracked_delta':[],'pending_rss_delta_bytes':[],'released_tracked_delta':[]}
   for r in samples:
    hashes.add(r['source_sha256']);hosts.add(tuple(r[k] for k in ['node','platform','arch','cpu']))
    phases={x['phase']:x for x in r['phases']};ready=phases['ready'];pending=phases['pending_collected'];released=phases['released']
    values['pending_tracked_delta'].append(pending['engine_stats']['tracked']-ready['engine_stats']['tracked'])
    values['pending_rss_delta_bytes'].append(pending['rss_bytes']-ready['rss_bytes'])
    values['released_tracked_delta'].append(released['engine_stats']['tracked']-ready['engine_stats']['tracked'])
   result['builds'][label]={k:{'samples':v,'median':statistics.median(v),'mad':statistics.median(abs(x-statistics.median(v)) for x in v)} for k,v in values.items()}
  assert len(hashes)==len(hosts)==1
  result['source_sha256']=hashes.pop()
  result['candidate_minus_before_tracked']=result['builds']['candidate']['pending_tracked_delta']['median']-result['builds']['before']['pending_tracked_delta']['median']
  results.append(result)
report={'schema':1,'kind':'pending_promise_memory_comparison','builds':records,'changed_inputs':changes,'script_sha256':sha(script),'runs':runs,'results':results,'limitations':['Per-owner tracked registry cells after explicit guest GC are not allocated bytes.','Each memory phase samples one process; RSS includes Node, code, builtins and allocator caches. Three process repetitions per case/build are retained.','Only native species can reuse a real result promise; custom species is the preserved-fallback control.','Sources and expected completion checks match across builds; timing experiments run separately.']}
out.write_text(json.dumps(report,indent=2)+'\n');print(out)
