#!/usr/bin/env python3
"""Build, measure and compare public native/WASM/Node source APIs."""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import statistics
import subprocess
import sys

ROOT=Path(__file__).resolve().parent.parent
SCRIPT=ROOT/'bench/performance/platforms.cjs'
ENGINES=('napi-native','napi-wasm','node-v8')
MODES=('fresh_source','warm_source')
STARTUP={'native_addon_load_ns':('napi-native','module_load'),
         'node_vm_builtin_load_ns':('node-v8','module_load'),
         'wasm_glue_import_ns':('napi-wasm','module_load'),
         'wasm_compile_ns':('napi-wasm','compile_module'),
         'wasm_instantiate_init_ns':('napi-wasm','instantiate_init')}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def package_digest(artifacts):
    files={p.removeprefix('wasm/'):h for p,h in artifacts.items() if p.startswith('wasm/')}
    return hashlib.sha256(json.dumps(files,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest()


def summary(values):
    if not values or any(type(v) not in (int,float) or not math.isfinite(v) or v<=0 for v in values):
        raise ValueError('missing, non-finite or non-positive public timing')
    mid=statistics.median(values)
    return {'median':mid,'mad':statistics.median(abs(v-mid) for v in values)}


def fixtures(data):
    if data.get('schema')!=1 or data.get('kind')!='guest_workloads' or not data.get('workloads'):
        raise ValueError('invalid exported workloads')
    result={}
    for w in data['workloads']:
        name=w.get('name')
        if not isinstance(name,str) or not name or name in result:
            raise ValueError('missing or duplicate workload')
        if not all(isinstance(w.get(k),str) and w[k] for k in ('setup','invocation','source')):
            raise ValueError('missing guest source')
        if w['source']!=w['setup']+'\n'+w['invocation'] or hashlib.sha256(w['source'].encode()).hexdigest()!=w.get('source_sha256'):
            raise ValueError('invalid guest source hash/boundary')
        if type(w.get('expected')) not in (int,float) or not math.isfinite(w['expected']):
            raise ValueError('invalid expected guest result')
        result[name]=w
    return result


def inputs():
    files=[]
    for folder in ('src','crates/napi-vm-core','crates/napi-vm-runtime'):
        files.extend(p for p in (ROOT/folder).rglob('*') if p.is_file() and 'target' not in p.parts and p.suffix!='.md')
    files.extend(ROOT/p for p in ('Cargo.toml','Cargo.lock','.cargo/config.toml','build.rs'))
    return {str(p.relative_to(ROOT)):sha(p) for p in sorted(set(files))}


def helpers():
    return {str(p.relative_to(ROOT)):sha(p) for p in (Path(__file__),SCRIPT,ROOT/'tools/ci/wasm-package.mjs')}


def artifact_hashes(out):
    return {str(p.relative_to(out)):sha(p) for p in [out/'napi-vm.node',out/'workloads.json',*sorted((out/'wasm').rglob('*'))] if p.is_file()}


def check_artifacts(directory,record):
    if record.get('kind')!='public_platform_build':
        raise ValueError('invalid platform build record')
    if any(sha(directory/name)!=digest for name,digest in record['artifacts'].items()):
        raise ValueError('platform build artifacts changed')


def stage(directory,out):
    """Reuse verified compiled bytes with explicit packaging/measurement provenance."""
    directory=directory.resolve();out=out.resolve()
    original=json.loads((directory/'build.json').read_text());check_artifacts(directory,original)
    if original['helpers'].get('bench/performance/platforms.cjs')!=sha(SCRIPT):
        raise ValueError('cannot stage builds made for a different timing script')
    out.mkdir(parents=True,exist_ok=False)
    for name in ('napi-vm.node','workloads.json'):shutil.copy2(directory/name,out/name)
    shutil.copytree(directory/'wasm',out/'wasm')
    command=['node',str(ROOT/'tools/ci/wasm-package.mjs'),'prepare',str(out/'wasm')]
    subprocess.run(command,cwd=ROOT,check=True)
    record={**original,'helpers':helpers(),'artifacts':artifact_hashes(out),
            'artifact_staging':{'original_build_record':original,'original_build_record_sha256':sha(directory/'build.json'),
                                'command':command,'notes':'Compiled addon/WASM/glue bytes are reused; only the package file whitelist may be extended.'}}
    if record['artifacts'].keys()!=original['artifacts'].keys():
        raise RuntimeError('staging added or removed runtime artifacts')
    for name,digest in original['artifacts'].items():
        if name!='wasm/package.json' and record['artifacts'][name]!=digest:
            raise RuntimeError('staging changed compiled runtime bytes')
    (out/'build.json').write_text(json.dumps(record,indent=2)+'\n')
    print(out)


def build(engine_build,out):
    out=out.resolve();out.mkdir(parents=True,exist_ok=False)
    original=json.loads((engine_build/'build.json').read_text())
    executable=engine_build/'comparison';fixture=engine_build/'workloads.json'
    if sha(executable)!=original['executable_sha256'] or sha(fixture)!=original.get('workloads_sha256'):
        raise ValueError('engine export artifacts do not match their build record')
    emitted=json.loads(subprocess.check_output([str(executable.resolve()),'--emit-workloads'],text=True))
    if emitted!=json.loads(fixture.read_text()):
        raise ValueError('workload file does not match immutable exporter')
    fixtures(emitted)
    before=inputs();helper_hashes=helpers()
    commands=[['cargo','build','--locked','--release','--lib'],
              ['node','tools/ci/wasm-package.mjs','build','--out-dir',str(out/'wasm')]]
    for index,command in enumerate(commands):
        with (out/('native-build.log' if index==0 else 'wasm-build.log')).open('w') as log:
            subprocess.run(command,cwd=ROOT,check=True,stdout=log,stderr=subprocess.STDOUT)
        if index==0:
            shutil.copy2(ROOT/'target/release/libnapi_vm.so',out/'napi-vm.node')
    if inputs()!=before or helpers()!=helper_hashes:
        raise RuntimeError('platform build inputs changed during compilation')
    shutil.copy2(fixture,out/'workloads.json')
    artifacts=artifact_hashes(out)
    record={'schema':1,'kind':'public_platform_build','utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'inputs':before,'helpers':helper_hashes,'artifacts':artifacts,'commands':commands,
            'profiles':{'native':'release; default napi feature','wasm':'release; no-default-features; wasm,runtime; web target'},
            'rustc':subprocess.check_output(['rustc','--version','--verbose'],text=True),
            'wasm_pack':subprocess.check_output(['wasm-pack','--version'],text=True).strip(),
            'build_environment':{k:v for k,v in os.environ.items() if k in ('RUSTFLAGS','RUSTC_WRAPPER','CARGO_BUILD_TARGET','RUSTUP_TOOLCHAIN','CARGO_TARGET_DIR') or k.startswith(('CARGO_PROFILE_','CARGO_TARGET_'))},
            'fixture_exporter':original}
    (out/'build.json').write_text(json.dumps(record,indent=2)+'\n')
    print(out)


def matrix(workloads):
    result={(engine,mode,name):(w['source_sha256'],'ns/call')
            for name,w in workloads.items() for engine in ENGINES for mode in MODES}
    result.update({(engine,'construct_dispose','vm_lifecycle'):(None,'ns/instance') for engine in ENGINES})
    return result


def validate_child(run,workloads,samples,iterations,hashes):
    if any(type(run.get(k)) is not int for k in ('schema','samples','iterations','warmup_calls')):
        raise ValueError('invalid child platform numeric settings')
    if (run.get('schema'),run.get('kind'),run.get('samples'),run.get('iterations'),run.get('warmup_calls'))!=(1,'public_platform_timing',samples,iterations,5):
        raise ValueError('invalid child platform settings')
    if fixtures({'schema':1,'kind':'guest_workloads','workloads':run.get('workloads')})!=workloads:
        raise ValueError('child platform workload matrix changed')
    if run['metadata'].get('hashes')!=hashes or run['metadata'].get('host_gc') is not True:
        raise ValueError('child platform artifact hash/GC mismatch')
    expected=matrix(workloads);rows={}
    for row in run.get('results',[]):
        key=tuple(row.get(k) for k in ('engine','mode','workload'))
        if key not in expected or key in rows or (row.get('source_sha256'),row.get('unit'))!=expected[key]:
            raise ValueError('unknown, duplicate or changed public platform row')
        values=row.get('samples',[])
        stored={k:row.get(k) for k in ('median','mad')}
        if any(type(v) not in (int,float) or not math.isfinite(v) for v in stored.values()):
            raise ValueError('invalid child platform stored summary')
        if len(values)!=samples or summary(values)!=stored:
            raise ValueError('invalid child platform samples/summary')
        rows[key]=row
    if rows.keys()!=expected.keys() or set(run.get('startup',{}))!=set(STARTUP):
        raise ValueError('incomplete child platform matrix')
    summary(list(run['startup'].values()))
    return rows


def aggregate(runs,workloads):
    keys=matrix(workloads);rows=[]
    for key,(source,unit) in keys.items():
        values=[next(r for r in run['results'] if (r['engine'],r['mode'],r['workload'])==key)['median'] for run in runs]
        rows.append(dict(zip(('engine','mode','workload'),key),unit=unit,source_sha256=source,samples=values,**summary(values)))
    for field,(engine,mode) in STARTUP.items():
        values=[run['startup'][field] for run in runs]
        rows.append(dict(engine=engine,mode=mode,workload='startup',unit='ns/component',source_sha256=None,samples=values,**summary(values)))
    return rows


def validate(report):
    if report.get('schema')!=1 or report.get('kind')!='public_platforms':
        raise ValueError('invalid public platform report')
    for key in ('samples','iterations','processes'):
        if type(report.get(key)) is not int or report[key]<=0 or (key!='iterations' and report[key]%2!=1):
            raise ValueError('invalid public platform '+key)
    workloads=fixtures(report['fixtures'])
    runs=report.get('runs',[])
    if len(runs)!=report['processes']:
        raise ValueError('incomplete process repetitions')
    hashes=report['metadata']['hashes']
    if not isinstance(hashes,dict) or set(hashes)!=set(('addon','wasm','glue','package','workloads','script')) or any(not isinstance(v,str) or not re.fullmatch('[0-9a-f]{64}',v) for v in hashes.values()):
        raise ValueError('invalid public platform artifact digests')
    meta=report['metadata']
    if any(not isinstance(meta.get(k),str) or not meta[k] for k in ('node','v8','platform','arch','cpu','helper_sha256')):
        raise ValueError('missing public platform host/helper metadata')
    build=meta.get('build',{})
    if build.get('schema')!=1 or build.get('kind')!='public_platform_build':
        raise ValueError('missing public platform build provenance')
    for artifact,key in (('napi-vm.node','addon'),('wasm/napi_vm_bg.wasm','wasm'),('wasm/napi_vm.js','glue'),('workloads.json','workloads')):
        if build.get('artifacts',{}).get(artifact)!=hashes[key]:
            raise ValueError('public platform artifacts differ from build provenance')
    if package_digest(build['artifacts'])!=hashes['package']:
        raise ValueError('public platform package tree differs from build provenance')
    if build.get('helpers',{}).get('bench/platforms.py')!=meta['helper_sha256'] or build['helpers'].get('bench/performance/platforms.cjs')!=hashes['script']:
        raise ValueError('public platform helpers differ from build provenance')
    for run in runs:
        validate_child(run,workloads,report['samples'],report['iterations'],hashes)
        for key in ('node','v8','platform','arch','cpu'):
            if run['metadata'].get(key)!=report['metadata'].get(key):
                raise ValueError('inconsistent child platform host')
    if report.get('results')!=aggregate(runs,workloads):
        raise ValueError('invalid platform aggregation')
    return workloads


def run(directory,destination,samples,iterations,processes):
    if destination.exists():
        raise ValueError('output must be new')
    directory=directory.resolve()
    record=json.loads((directory/'build.json').read_text())
    if record.get('kind')!='public_platform_build' or record.get('helpers')!=helpers():
        raise ValueError('platform helper/build record mismatch; rebuild artifacts')
    check_artifacts(directory,record)
    data=json.loads((directory/'workloads.json').read_text());workloads=fixtures(data)
    hashes={'addon':sha(directory/'napi-vm.node'),'wasm':sha(directory/'wasm/napi_vm_bg.wasm'),
            'glue':sha(directory/'wasm/napi_vm.js'),'package':package_digest(record['artifacts']),
            'workloads':sha(directory/'workloads.json'),'script':sha(SCRIPT)}
    runs=[]
    for _ in range(processes):
        raw=subprocess.check_output(['node','--expose-gc',str(SCRIPT)],cwd=ROOT,text=True,
            env={**os.environ,'NAPI_BENCH_BINDING':str(directory/'napi-vm.node'),'NAPI_VM_WASM_PACKAGE':str(directory/'wasm'),
                 'PLATFORM_BENCH_WORKLOADS':str(directory/'workloads.json'),'PLATFORM_BENCH_SAMPLES':str(samples),'PLATFORM_BENCH_ITERATIONS':str(iterations)})
        child=json.loads(raw);validate_child(child,workloads,samples,iterations,hashes);runs.append(child)
    report={'schema':1,'kind':'public_platforms','samples':samples,'iterations':iterations,'processes':processes,
            'fixtures':data,'runs':runs,'results':aggregate(runs,workloads),
            'metadata':{**{k:runs[0]['metadata'][k] for k in ('node','v8','platform','arch','cpu')},
                        'hashes':hashes,'build':record,'helper_sha256':sha(Path(__file__)),
                        'limitations':runs[0]['metadata']['limitations']+['Aggregate timings are medians of independent process medians; full child batch samples are retained.']}}
    validate(report)
    destination.parent.mkdir(parents=True,exist_ok=True)
    destination.write_text(json.dumps(report,indent=2)+'\n')
    print(destination)


def compare(before,after):
    old,new=[json.loads(p.read_text()) for p in (before,after)]
    if validate(old)!=validate(new):
        raise ValueError('incompatible platform fixtures')
    for key in ('samples','iterations','processes'):
        if old[key]!=new[key]:raise ValueError('incompatible platform '+key)
    for key in ('node','v8','platform','arch','cpu','helper_sha256'):
        if old['metadata'][key]!=new['metadata'][key]:raise ValueError('incompatible platform '+key)
    builds=[r['metadata']['build'] for r in (old,new)]
    if old['metadata']['hashes']['workloads']!=new['metadata']['hashes']['workloads']:
        raise ValueError('incompatible platform exported fixture bytes')
    for key in ('helpers','profiles','rustc','wasm_pack','build_environment'):
        if builds[0][key]!=builds[1][key]:raise ValueError('incompatible platform build '+key)
    fixed=('Cargo.toml','Cargo.lock','.cargo/config.toml','build.rs','crates/napi-vm-core/Cargo.toml','crates/napi-vm-runtime/Cargo.toml')
    if any(builds[0]['inputs'][p]!=builds[1]['inputs'][p] for p in fixed):
        raise ValueError('incompatible platform dependency/configuration inputs')
    results=[]
    right={(r['engine'],r['mode'],r['workload']):r for r in new['results']}
    for x in old['results']:
        y=right[(x['engine'],x['mode'],x['workload'])]
        results.append({**{k:x[k] for k in ('engine','mode','workload','unit')},
            'before_median':x['median'],'after_median':y['median'],'after_over_before':y['median']/x['median']})
    return {'schema':1,'results':results,'changed_inputs':[p for p,h in builds[0]['inputs'].items() if builds[1]['inputs'].get(p)!=h],
            'limitations':new['metadata']['limitations']}


def main():
    p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='command',required=True)
    b=sub.add_parser('build');b.add_argument('--engine-build',type=Path,required=True);b.add_argument('--output',type=Path,required=True)
    s=sub.add_parser('stage');s.add_argument('--build',type=Path,required=True);s.add_argument('--output',type=Path,required=True)
    r=sub.add_parser('run');r.add_argument('--build',type=Path,required=True);r.add_argument('--output',type=Path,required=True)
    r.add_argument('--samples',type=int,default=7);r.add_argument('--iterations',type=int,default=3);r.add_argument('--processes',type=int,default=3)
    c=sub.add_parser('compare');c.add_argument('before',type=Path);c.add_argument('after',type=Path)
    args=p.parse_args()
    try:
        if args.command=='build':build(args.engine_build.resolve(),args.output)
        elif args.command=='stage':stage(args.build,args.output)
        elif args.command=='run':
            if args.samples<=0 or args.samples%2!=1 or args.iterations<=0 or args.processes<=0 or args.processes%2!=1:
                p.error('samples/processes must be positive and odd; iterations positive')
            run(args.build,args.output,args.samples,args.iterations,args.processes)
        else:print(json.dumps(compare(args.before,args.after),indent=2))
    except (OSError,ValueError,RuntimeError,subprocess.CalledProcessError) as error:
        p.exit(1,str(error)+'\n')


if __name__=='__main__':main()
