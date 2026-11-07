#!/usr/bin/env python3
"""Measure Intl/Temporal only when an API exists and its checked result matches."""
import argparse
import datetime
import json
import hashlib
import math
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
from async_jobs import sha, summary, fixture_digest

ROOT=Path(__file__).resolve().parent.parent
ENGINES=('napi-vm','rquickjs','node-v8-context')
MODES=('fresh_source','warm_source','warm_call')
SOURCE=ROOT/'bench/international-workloads.json'
SCRIPT=ROOT/'bench/performance/international.cjs'


def inputs():
    paths=[p for folder in ('crates/napi-vm-core','bench/rquickjs') for p in (ROOT/folder).rglob('*') if p.is_file() and 'target' not in p.parts and p.suffix!='.md']
    paths.extend((Path(__file__),SOURCE,SCRIPT,ROOT/'bench/async_jobs.py',ROOT/'.cargo/config.toml'))
    return {str(p.relative_to(ROOT)):sha(p) for p in sorted(set(paths))}


def fixtures(data):
    if type(data.get('schema')) is not int or data['schema']!=1 or data.get('kind')!='international_workloads' or not isinstance(data.get('probe_source'),str) or not data['probe_source']:
        raise ValueError('invalid international fixture export')
    result={}
    for w in data.get('workloads',[]):
        name=w.get('name')
        if not isinstance(name,str) or not name or name in result:raise ValueError('missing or duplicate international fixture')
        if not all(isinstance(w.get(k),str) and w[k] for k in ('setup','source','invocation')):raise ValueError('missing international source')
        if w['source']!=w['setup']+'\n'+w['invocation'] or hashlib.sha256(w['source'].encode()).hexdigest()!=w.get('source_sha256'):raise ValueError('international source/hash changed')
        if type(w.get('expected')) not in (int,float):raise ValueError('invalid international expected value')
        if not math.isfinite(w['expected']):raise ValueError('nonfinite international result')
        requires=w.get('requires')
        if not isinstance(requires,list) or not requires or any(not isinstance(x,str) or not re.fullmatch(r'(Intl|Temporal)\.[A-Za-z]+',x) for x in requires) or len(set(requires))!=len(requires):raise ValueError('invalid capability requirements')
        result[name]=w
    if not result:raise ValueError('empty international fixture export')
    return result


def build(out):
    out=out.resolve();out.mkdir(parents=True,exist_ok=False);before=inputs()
    command=['cargo','build','--locked','--release','--jobs','2','--manifest-path',str(ROOT/'bench/rquickjs/Cargo.toml'),'--target-dir',str(ROOT/'target/rquickjs-comparison'),'--bin','international']
    with (out/'build.log').open('w') as log:subprocess.run(command,cwd=ROOT,check=True,stdout=log,stderr=subprocess.STDOUT)
    if inputs()!=before:raise RuntimeError('international inputs changed during compilation')
    shutil.copy2(ROOT/'target/rquickjs-comparison/release/international',out/'worker');shutil.copy2(SCRIPT,out/'node.cjs')
    raw=subprocess.check_output([str(out/'worker'),'--emit-workloads'],text=True);data=json.loads(raw);fixtures(data);(out/'workloads.json').write_text(raw)
    record=dict(schema=1,kind='international_build',inputs=before,command=command,driver_sha256=sha(Path(__file__)),fixture_matrix_sha256=fixture_digest(data),artifacts={n:sha(out/n) for n in ('worker','node.cjs','workloads.json')},rustc=subprocess.check_output(['rustc','--version','--verbose'],text=True),profile='release; opt-level=3; thin LTO; codegen-units=1; default core/rquickjs std; Context::full',environment={k:v for k,v in os.environ.items() if k in ('RUSTFLAGS','RUSTC_WRAPPER','RUSTUP_TOOLCHAIN','CARGO_BUILD_TARGET','CARGO_ENCODED_RUSTFLAGS','CC','AR','CFLAGS','CXXFLAGS') or k.startswith(('CARGO_PROFILE_','CARGO_TARGET_'))})
    (out/'build.json').write_text(json.dumps(record,indent=2)+'\n');print(out)


def validate_child(run,workloads,samples,iterations,engines):
    if any(type(run.get(k)) is not int for k in ('schema','samples','iterations','warmup_calls')) or (run['schema'],run.get('kind'),run['samples'],run['iterations'],run['warmup_calls'])!=(1,'international_child',samples,iterations,5):raise ValueError('invalid international child settings')
    caps=run.get('capabilities',{});names={c for w in workloads.values() for c in w['requires']}
    if set(caps)!=set(engines) or any(set(caps[e])!=names or any(type(v) is not bool for v in caps[e].values()) for e in engines):raise ValueError('incomplete international capability survey')
    rows={};outcomes={}
    for row in run.get('results',[]):
        key=tuple(row.get(k) for k in ('engine','mode','workload'));engine,mode,name=key
        if engine not in engines or mode not in MODES or name not in workloads or key in rows:raise ValueError('unknown or duplicate international row')
        w=workloads[name];missing=[c for c in w['requires'] if not caps[engine][c]]
        if row.get('source_sha256')!=w['source_sha256'] or row.get('unit')!='ns/call' or row.get('missing')!=missing:raise ValueError('international boundary/source/capability mismatch')
        status=row.get('status')
        if missing:
            if status!='unsupported':raise ValueError('absent capability has timing')
        elif status not in ('measured','incompatible_result'):raise ValueError('available capability silently omitted')
        if status=='measured':
            if len(row.get('samples',[]))!=samples or summary(row['samples'])!={k:row.get(k) for k in ('median','mad')}:raise ValueError('invalid international samples/statistics')
            if any(type(row.get(k)) not in (int,float) for k in ('median','mad')):raise ValueError('invalid international statistic type')
            if 'diagnostic' in row:raise ValueError('measured result also marked incompatible')
        else:
            if any(k in row for k in ('samples','median','mad')):raise ValueError('non-equivalent or unsupported result has fabricated timing')
            if status=='incompatible_result' and (not isinstance(row.get('diagnostic'),str) or not row['diagnostic']):raise ValueError('missing semantic diagnostic')
        outcome=(status,tuple(missing),row.get('diagnostic'))
        if outcomes.setdefault((engine,name),outcome)!=outcome:raise ValueError('inconsistent international preflight outcome')
        rows[key]=row
    if len(rows)!=len(engines)*len(MODES)*len(workloads):raise ValueError('incomplete international matrix')
    return rows


def aggregate(runs,workloads):
    indexed=[]
    for pair in runs:
        indexed.append({(x['engine'],x['mode'],x['workload']):x for child in pair.values() for x in child['results']})
    result=[]
    for name in workloads:
        for mode in MODES:
            for engine in ENGINES:
                key=(engine,mode,name);first=indexed[0][key];row={k:v for k,v in first.items() if k not in ('samples','median','mad')}
                if any({k:v for k,v in r[key].items() if k not in ('samples','median','mad')}!=row for r in indexed):raise ValueError('international outcome changed between processes')
                if first['status']=='measured':
                    values=[r[key]['median'] for r in indexed];row.update(samples=values,**summary(values))
                result.append(row)
    return result


def validate(report):
    if type(report.get('schema')) is not int or report['schema']!=1 or report.get('kind')!='international':raise ValueError('invalid international report')
    for key in ('samples','iterations','processes'):
        if type(report.get(key)) is not int or report[key]<=0 or key!='iterations' and report[key]%2!=1:raise ValueError('invalid international settings')
    workloads=fixtures(report['fixtures']);runs=report.get('runs',[])
    if len(runs)!=report['processes']:raise ValueError('missing international process runs')
    meta=report['metadata'];b=meta['build']
    if b.get('kind')!='international_build' or meta['driver_sha256']!=b['driver_sha256'] or fixture_digest(report['fixtures'])!=b['fixture_matrix_sha256']:raise ValueError('international provenance mismatch')
    if set(b['artifacts'])!={'worker','node.cjs','workloads.json'} or any(not isinstance(v,str) or not re.fullmatch('[a-f0-9]{64}',v) for v in b['artifacts'].values()):raise ValueError('invalid international artifact hashes')
    for pair in runs:
        if set(pair)!={'rust','node'}:raise ValueError('missing international backend process')
        validate_child(pair['rust'],workloads,report['samples'],report['iterations'],ENGINES[:2]);validate_child(pair['node'],workloads,report['samples'],report['iterations'],ENGINES[2:])
        if pair['rust']['capabilities']!=runs[0]['rust']['capabilities'] or pair['node']['capabilities']!=runs[0]['node']['capabilities']:raise ValueError('international capabilities changed between runs')
        node=pair['node']['metadata']
        if node['fixtures_sha256']!=b['artifacts']['workloads.json'] or node['script_sha256']!=b['artifacts']['node.cjs'] or node.get('host_gc') is not True:raise ValueError('international Node helper/fixture/GC changed')
        for key in ('node','v8','icu','platform','arch','cpu'):
            if node.get(key)!=meta.get(key):raise ValueError('international host changed between runs')
    if report.get('results')!=aggregate(runs,workloads):raise ValueError('invalid international aggregation')
    return workloads


def run(directory,out,samples,iterations,processes):
    directory=directory.resolve()
    if out.exists():raise ValueError('output must be new')
    b=json.loads((directory/'build.json').read_text())
    if b['driver_sha256']!=sha(Path(__file__)) or sha(ROOT/'bench/async_jobs.py')!=b['inputs']['bench/async_jobs.py'] or any(sha(directory/n)!=h for n,h in b['artifacts'].items()):raise ValueError('international artifacts/helper changed')
    data=json.loads((directory/'workloads.json').read_text());workloads=fixtures(data);runs=[];loads=[]
    for _ in range(processes):
        loads.append(os.getloadavg() if hasattr(os,'getloadavg') else None)
        rust=json.loads(subprocess.check_output([str(directory/'worker'),'--samples',str(samples),'--iterations',str(iterations)],text=True,cwd=ROOT))
        node=json.loads(subprocess.check_output(['node','--expose-gc',str(directory/'node.cjs')],env={**os.environ,'INTERNATIONAL_FIXTURES':str(directory/'workloads.json'),'INTERNATIONAL_SAMPLES':str(samples),'INTERNATIONAL_ITERATIONS':str(iterations)},text=True,cwd=ROOT))
        validate_child(rust,workloads,samples,iterations,ENGINES[:2]);validate_child(node,workloads,samples,iterations,ENGINES[2:]);runs.append(dict(rust=rust,node=node))
    report=dict(schema=1,kind='international',samples=samples,iterations=iterations,processes=processes,fixtures=data,runs=runs,results=aggregate(runs,workloads),metadata={**{k:runs[0]['node']['metadata'][k] for k in ('node','v8','icu','platform','arch','cpu')},'build':b,'driver_sha256':sha(Path(__file__)),'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'load_average_at_process_start':loads,'affinity':sorted(os.sched_getaffinity(0)) if hasattr(os,'sched_getaffinity') else None,'run_environment':{k:os.environ[k] for k in ('NODE_OPTIONS','TZ','LANG','LC_ALL') if k in os.environ},'limitations':['Finite synchronous Intl/Temporal fixtures with explicit en-US/UTC/fixed inputs. API presence alone does not establish equivalent behavior.','Unsupported and incompatible-result rows retain reasons and have no timing samples; they are not speedups.','Rust backends and Node run in separate processes. Each result aggregates independent process medians; raw child batches are retained.','Fresh includes unique-comment source allocation, construction/builtins, evaluation and release. Rust napi-vm includes explicit guest cycle GC; Node releases context references and host GC runs outside batches.','Warm source preserves napi-vm source caches and uses each engine source API; warm call retains a guest function. Nested tiers are not guaranteed identical.','Assertions run outside batch timers; numeric result conversion and result summation are timed. Five warmups, no CPU isolation and no universal runtime score.']})
    validate(report);out.parent.mkdir(parents=True,exist_ok=True);out.write_text(json.dumps(report,indent=2)+'\n');print(out)


def compare(a,b):
    before,after=(json.loads(p.read_text()) for p in (a,b))
    if validate(before)!=validate(after):raise ValueError('international fixtures differ')
    for k in ('samples','iterations','processes'):
        if before[k]!=after[k]:raise ValueError('international settings differ')
    for k in ('cpu','platform','arch','node','v8','icu','affinity','run_environment','driver_sha256'):
        if before['metadata'][k]!=after['metadata'][k]:raise ValueError('international host/helper differs')
    builds=[r['metadata']['build'] for r in (before,after)]
    for k in ('rustc','profile','environment'):
        if builds[0][k]!=builds[1][k]:raise ValueError('international build configuration differs')
    left,right=(x['inputs'] for x in builds);changed=sorted(k for k in set(left)|set(right) if left.get(k)!=right.get(k))
    if any(not k.startswith('crates/napi-vm-core/src/') for k in changed):raise ValueError('international harness/dependency/configuration changed')
    rows={(x['engine'],x['mode'],x['workload']):x for x in after['results']};results=[]
    for x in before['results']:
        y=rows[x['engine'],x['mode'],x['workload']];r={k:x[k] for k in ('engine','mode','workload')};r.update(before_status=x['status'],after_status=y['status'])
        if x['status']==y['status']=='measured':r.update(before=x['median'],after=y['median'],after_over_before=y['median']/x['median'])
        results.append(r)
    return dict(schema=1,changed_inputs=changed,results=results)


def ratios(path):
    data=json.loads(path.read_text());validate(data);rows={(x['engine'],x['mode'],x['workload']):x for x in data['results']};result=[]
    for w in data['fixtures']['workloads']:
        for mode in MODES:
            for other in ENGINES[1:]:
                a,b=rows['napi-vm',mode,w['name']],rows[other,mode,w['name']];r=dict(workload=w['name'],mode=mode,reference=other,napi_status=a['status'],reference_status=b['status'])
                if a['status']==b['status']=='measured':r.update(napi_vm=a['median'],reference_median=b['median'],napi_vm_over_reference=a['median']/b['median'])
                result.append(r)
    return dict(schema=1,source_report_sha256=sha(path),results=result,capabilities={**data['runs'][0]['rust']['capabilities'],**data['runs'][0]['node']['capabilities']},limitations=data['metadata']['limitations'])


def main():
    p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='command',required=True)
    b=sub.add_parser('build');b.add_argument('--output',type=Path,required=True)
    r=sub.add_parser('run');r.add_argument('--build',type=Path,required=True);r.add_argument('--output',type=Path,required=True)
    r.add_argument('--samples',type=int,default=15);r.add_argument('--iterations',type=int,default=10);r.add_argument('--processes',type=int,default=3)
    c=sub.add_parser('compare');c.add_argument('before',type=Path);c.add_argument('after',type=Path)
    q=sub.add_parser('ratios');q.add_argument('report',type=Path)
    a=p.parse_args()
    try:
        if a.command=='build':build(a.output)
        elif a.command=='run':
            if a.samples<=0 or a.samples%2!=1 or a.iterations<=0 or a.processes<=0 or a.processes%2!=1:p.error('positive settings required; samples/processes must be odd')
            run(a.build,a.output,a.samples,a.iterations,a.processes)
        else:print(json.dumps(compare(a.before,a.after) if a.command=='compare' else ratios(a.report),indent=2))
    except (OSError,ValueError,RuntimeError,subprocess.CalledProcessError) as e:p.exit(1,str(e)+'\n')


if __name__=='__main__':main()
