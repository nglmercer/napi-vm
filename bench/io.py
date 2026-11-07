#!/usr/bin/env python3
"""Freeze and measure local filesystem, source loader and TypeScript boundaries."""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
FS = ['read_bytes_empty','read_bytes_4k','read_bytes_64k','read_bytes_4m',
      'read_bytes_nested_4k','read_text_64k','write_bytes_4k','mkdir_existing_nested']
NATIVE = {('permissions',n) for n in FS+['denied_read']} | {('rust_std',n) for n in FS} | {
    ('file_loader','resolve'),('file_loader','load_64k'),('npm_loader','resolve_4m'),
    ('npm_loader','cjs_resolve_read_64k'),('oxc','typescript_100_sourcemap'),('oxc','typescript_1000_sourcemap')}
NODE = {('node_fs',n) for n in FS} | {('node_file','resolve'),('node_file','load_64k'),('node_cjs','cjs_resolve_read_64k')}
HARNESS = ['bench/io.py','bench/performance/io.cjs','crates/napi-vm-runtime/examples/io_bench.rs',
           'crates/napi-vm-runtime/examples/io_allocations.rs','crates/napi-vm-runtime/examples/io_support/mod.rs']

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def command(args, **kwargs):
    return subprocess.run([str(x) for x in args], cwd=ROOT, check=True, text=True, **kwargs)

def output(args):
    return command(args, stdout=subprocess.PIPE).stdout.strip()

def inputs():
    paths = [ROOT/n for n in ['Cargo.toml','Cargo.lock','.cargo/config.toml']+HARNESS]
    paths += [p for folder in ['crates/napi-vm-core','crates/napi-vm-runtime'] for p in (ROOT/folder).rglob('*')
              if p.is_file() and 'target' not in p.parts and p.suffix != '.md']
    return {str(p.relative_to(ROOT)):sha(p) for p in sorted(set(paths))}

def fixtures(root):
    root.mkdir()
    for relative, content in {
        'empty':b'', '4k':b'x'*4096,'64k':b'x'*65536,'4m':b'x'*(4<<20),'a/b/c/d/4k':b'x'*4096,
        'package.json':b'{"type":"module"}',
        'node_modules/perf-local/package.json':b'{"name":"perf-local","exports":{".":{"import":"./esm.mjs","require":"./cjs.cjs"}}}',
        'module.mjs':b'export const answer=42;\n//'+b'x'*(65536-26),
        'node_modules/perf-local/esm.mjs':b'export const answer=42;\n//'+b'x'*((4<<20)-26),
        'node_modules/perf-local/cjs.cjs':b'module.exports={answer:42};\n//'+b'x'*(65536-30),
        **{f'types-{n}.ts':('interface Item { value:number }\n'+''.join(f'const v{i}:Item={{value:{i}}};\n' for i in range(n))+f'v{n-1}.value;').encode() for n in [100,1000]},
    }.items():
        path=root/relative; path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(content)
    immutable={str(p.relative_to(root)):sha(p) for p in sorted(root.rglob('*')) if p.is_file()}
    for backend in ['permissions','rust_std','node_fs']:
        (root/('write-'+backend)).write_bytes(b'')
    return immutable

def build(directory):
    directory.mkdir(parents=True,exist_ok=False)
    start=inputs()
    args=['cargo','build','--locked','--profile','ci-optimized','-p','napi-vm-runtime',
          '--features','runtime-fs,runtime-npm,runtime-typescript','--example','io_bench','--example','io_allocations']
    with (directory/'build.log').open('w') as log:
        command(args,stdout=log,stderr=subprocess.STDOUT)
    if inputs()!=start: raise RuntimeError('I/O build inputs changed during compilation')
    target=Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))
    files={}
    for source,name in [(target/'ci-optimized/examples/io_bench','io-bench'),(target/'ci-optimized/examples/io_allocations','io-allocations'),(ROOT/'bench/performance/io.cjs','io.cjs')]:
        shutil.copy2(source,directory/name);files[name]=sha(directory/name)
    record={'inputs':start,'files':files,'harness':{n:start[n] for n in HARNESS},
            'fixtures':fixtures(directory/'fixtures'),'command':args,'profile':'ci-optimized',
            'rustc':output(['rustc','--version','--verbose']), 'node':output(['node','--version']),
            'node_executable_sha256':sha(Path(shutil.which('node')).resolve()),
            'git_head':output(['git','rev-parse','HEAD']),
            'build_environment':{k:v for k,v in os.environ.items() if k in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTUP_TOOLCHAIN','CARGO_TARGET_DIR'] or k.startswith(('CARGO_PROFILE_','CARGO_TARGET_'))}}
    (directory/'build.json').write_text(json.dumps(record,indent=2)+'\n')
    print(directory)

def verify(directory,record):
    for name,digest in record['files'].items():
        if sha(directory/name)!=digest: raise ValueError('executable/helper digest mismatch')
    for name,digest in record['fixtures'].items():
        if sha(directory/'fixtures'/name)!=digest: raise ValueError('fixture digest mismatch')
    if sha(Path(shutil.which('node')).resolve())!=record['node_executable_sha256']: raise ValueError('Node executable changed')
    if record['harness']!={n:sha(ROOT/n) for n in HARNESS}: raise ValueError('I/O driver/harness differs from frozen build')

def summary(values):
    median=statistics.median(values)
    return {'median':median,'mad':statistics.median(abs(x-median) for x in values)}

def run(directory,destination,samples,iterations,processes):
    if destination.exists(): raise ValueError('output must be new')
    record=json.loads((directory/'build.json').read_text());verify(directory,record)
    rows={}; raw=[]; load=os.getloadavg()
    def capture(name,kind,process):
        args=([directory/name] if name!='io.cjs' else ['node',directory/name])+[directory/'fixtures',samples,iterations]
        result=json.loads(output(args));raw.append({'executable':name,'process':process,'report':result})
        expected=NODE if name=='io.cjs' else NATIVE
        keys=[(r['backend'],r['name']) for r in result['results']]
        if result['schema']!=1 or result['kind']!=kind or result['samples']!=samples or result['iterations']!=iterations or len(keys)!=len(expected) or set(keys)!=expected:
            raise ValueError('invalid I/O subprocess matrix')
        for item in result['results']:
            key=(kind,item['backend'],item['name'])
            if key not in rows:
                rows[key]={k:v for k,v in item.items() if k not in ['samples','requested_bytes_per_op']}
                rows[key].update(kind=kind,process_samples=[],requested_bytes_process_samples=[])
            rows[key]['process_samples'].append(item['samples'])
            rows[key]['requested_bytes_process_samples'].append(item['requested_bytes_per_op'])
    for process in range(processes):
        for name in (['io-bench','io.cjs'] if process%2==0 else ['io.cjs','io-bench']):capture(name,'latency',process)
    # Allocation counters never instrument the latency executable.
    for process in range(processes):capture('io-allocations','allocations',process)
    for row in rows.values():
        values=[v for p in row['process_samples'] for v in p]
        row['summary']=summary(values)
        if row['kind']=='allocations':row['requested_bytes_summary']=summary([v for p in row['requested_bytes_process_samples'] for v in p])
    verify(directory,record)
    result={'schema':1,'samples':samples,'iterations':iterations,'processes':processes,
            'results':list(rows.values()),'raw_processes':raw,
            'metadata':{'build':record,'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
                        'platform':platform.platform(),'load_average_start':load,'load_average_end':os.getloadavg(),
                        'limitations':['Warm page cache; five warmups per case; no cold disk, fsync or concurrent traffic.',
                            'Native System-allocator timings and counting-allocator diagnostics use separate executables.',
                            'Permission calls traverse no-follow descriptors and reject aliases; std/Node references do not enforce this policy.',
                            'Read buffers, UTF-8 validation, path processing and deallocation are timed; input payload/grant construction is excluded.',
                            'Assertions of returned lengths remain in timed loops; full byte equality and transformed-JS evaluation are checked before timing.',
                            'File URL resolution does not read files; npm resolution probes read file contents and manifests.',
                            'Node CJS resolution uses the warmed Node path cache, followed by a fresh read; Rust resolves and returns source each call.',
                            'Oxc transformation includes semantic analysis, JS and separate source-map generation; no equivalent rquickjs or Node transform is claimed.',
                            'mkdir measures an existing path; no guest evaluation, module graph execution, install/network, OS cache dropping, or async I/O.',
                            'Workloads have a fixed order within each process; native/Node process order alternates. No CPU isolation or aggregate runtime score.']}}
    destination.parent.mkdir(parents=True,exist_ok=True);destination.write_text(json.dumps(result,indent=2)+'\n')
    read_report(destination)
    print(destination)

def read_report(path):
    report=json.loads(path.read_text())
    for name in ['samples','iterations','processes']:
        value=report.get(name)
        if type(value)!=int or value<=0 or name!='iterations' and value%2!=1:raise ValueError('invalid I/O settings')
    if report.get('schema')!=1:raise ValueError('invalid I/O schema')
    rows={}
    expected={('latency',b,n) for b,n in NATIVE|NODE}|{('allocations',b,n) for b,n in NATIVE}
    for row in report['results']:
        key=(row['kind'],row['backend'],row['name'])
        if key in rows or key not in expected:raise ValueError('duplicate or unexpected I/O row')
        unit='ns/op' if row['kind']=='latency' else 'allocation requests/op'
        if row['unit']!=unit or type(row['checked_result'])!=int or row['checked_result']<0:raise ValueError('invalid I/O row')
        matrices=[('process_samples','summary')]
        if row['kind']=='allocations':matrices.append(('requested_bytes_process_samples','requested_bytes_summary'))
        elif row['requested_bytes_process_samples']!=[[] for _ in range(report['processes'])]:raise ValueError('latency mixed with allocation diagnostic')
        for samples_name,summary_name in matrices:
            groups=row[samples_name]
            if len(groups)!=report['processes'] or any(len(p)!=report['samples'] for p in groups):raise ValueError('incomplete I/O samples')
            values=[v for p in groups for v in p]
            if any(type(v) not in (int,float) or not math.isfinite(v) or v<0 or row['kind']=='latency' and v==0 for v in values):raise ValueError('invalid I/O sample')
            if any(type(row[summary_name].get(k)) not in (int,float) or not math.isclose(row[summary_name][k],v,rel_tol=1e-12,abs_tol=1e-12) for k,v in summary(values).items()):raise ValueError('inconsistent I/O summary')
        rows[key]=row
    if set(rows)!=expected:raise ValueError('incomplete I/O matrix')
    raw=report['raw_processes']
    rawkeys=[(p['executable'],p['process']) for p in raw]
    if len(set(rawkeys))!=len(rawkeys) or set(rawkeys)!={(name,i) for name in ['io-bench','io.cjs','io-allocations'] for i in range(report['processes'])}:raise ValueError('incomplete raw processes')
    for process in raw:
        native=process['executable']!='io.cjs';kind='allocations' if process['executable']=='io-allocations' else 'latency'
        data=process['report'];keys=[(r['backend'],r['name']) for r in data['results']]
        if data.get('schema')!=1 or data.get('kind')!=kind or any(data.get(n)!=report[n] for n in ['samples','iterations']) or len(keys)!=len(set(keys)) or set(keys)!=(NATIVE if native else NODE):raise ValueError('invalid raw I/O matrix')
        for r in data['results']:
            row=rows[(kind,r['backend'],r['name'])];i=process['process']
            if any(r[n]!=row[n] for n in ['unit','checked_result']) or r['samples']!=row['process_samples'][i] or r['requested_bytes_per_op']!=row['requested_bytes_process_samples'][i]:raise ValueError('raw I/O samples differ from summary inputs')
    return report,rows

def compare(before,after):
    a,left=read_report(before);b,right=read_report(after)
    if any(a[n]!=b[n] for n in ['samples','iterations','processes']):raise ValueError('I/O settings differ')
    x=a['metadata']['build'];y=b['metadata']['build']
    for n in ['harness','fixtures','command','profile','rustc','node','node_executable_sha256','build_environment']:
        if x[n]!=y[n]:raise ValueError('I/O comparison metadata differs: '+n)
    changes=sorted(k for k in x['inputs'].keys()|y['inputs'].keys() if x['inputs'].get(k)!=y['inputs'].get(k))
    print(json.dumps({'changed_inputs':changes,'results':[{'kind':k[0],'backend':k[1],'name':k[2],
        'before':left[k]['summary'],'after':right[k]['summary'],
        'after_over_before':right[k]['summary']['median']/left[k]['summary']['median'] if left[k]['summary']['median'] else None} for k in sorted(left)]},indent=2))

def main():
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='action',required=True)
    p=sub.add_parser('build');p.add_argument('--output',type=Path,required=True)
    p=sub.add_parser('run');p.add_argument('--build',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
    for name,default in [('samples',15),('iterations',10),('processes',3)]:p.add_argument('--'+name,type=int,default=default)
    p=sub.add_parser('compare');p.add_argument('before',type=Path);p.add_argument('after',type=Path)
    args=parser.parse_args()
    if args.action=='build':build(args.output.resolve())
    elif args.action=='compare':compare(args.before,args.after)
    else:
        if min(args.samples,args.iterations,args.processes)<=0 or args.samples%2!=1 or args.processes%2!=1:parser.error('samples/processes must be positive and odd; iterations positive')
        run(args.build.resolve(),args.output.resolve(),args.samples,args.iterations,args.processes)

if __name__=='__main__':main()
