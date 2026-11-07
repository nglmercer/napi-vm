"""Isolated engine memory probes; imported by rquickjs.py's memory command."""
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess

ENGINES = ('napi-vm', 'rquickjs')
WORKLOADS = ('objects', 'cycles', 'unique_keys', 'live_cycles')

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def metrics(run):
    snapshots = run['snapshots']
    phases = {s['phase']: s for s in snapshots}
    baseline, ready = phases['baseline'], phases['ready']
    collected = [s for s in snapshots if s['phase'] == 'collected']
    peak = max(s['process_high_water_bytes'] for s in snapshots)
    return {'ready_rss_bytes': ready['rss_bytes'],
            'last_allocated_rss_bytes': phases['allocated']['rss_bytes'],
            'last_collected_rss_bytes': collected[-1]['rss_bytes'],
            'collected_growth_bytes': collected[-1]['rss_bytes'] - ready['rss_bytes'],
            'released_rss_bytes': phases['released']['rss_bytes'],
            'dropped_rss_bytes': phases['dropped']['rss_bytes'],
            'retained_after_drop_bytes': phases['dropped']['rss_bytes'] - baseline['rss_bytes'],
            'peak_rss_bytes': peak, 'peak_increment_bytes': peak - baseline['process_high_water_bytes'],
            'gc_median_ns': statistics.median(s['gc_ns'] for s in collected)}

def summary(runs):
    measurements = [metrics(run) for run in runs]
    result = {}
    for name in measurements[0]:
        values = [m[name] for m in measurements]
        median = statistics.median(values)
        result[name] = {'median': median, 'mad': statistics.median(abs(x-median) for x in values),
                        'min': min(values), 'max': max(values)}
    return result

def validate(report):
    if report.get('schema') != 1 or report.get('kind') != 'process_memory':
        raise ValueError('invalid memory report schema')
    for key in ['samples', 'iterations', 'objects_per_call']:
        if type(report.get(key)) is not int or report[key] <= 0 or (key == 'samples' and report[key] % 2 == 0):
            raise ValueError('invalid memory setting: '+key)
    expected = [('baseline', 0), ('ready', 0)]
    for batch in range(1, report['iterations']+1):
        expected.extend([('allocated', batch), ('collected', batch)])
    expected.extend([('released', report['iterations']), ('dropped', report['iterations'])])
    rows, sources = {}, {}
    for row in report['results']:
        key = row['engine'], row['workload']
        if key in rows or len(row['runs']) != report['samples']:
            raise ValueError('duplicate or incomplete memory runs')
        for run in row['runs']:
            if (run['engine'], run['workload']) != key or run['iterations'] != report['iterations'] or run['objects_per_call'] != report['objects_per_call']:
                raise ValueError('memory run settings mismatch')
            digest = hashlib.sha256(run['source'].encode()).hexdigest()
            if run['source_sha256'] != digest or sources.setdefault(key[1], digest) != digest:
                raise ValueError('memory workload source mismatch')
            snapshots = run['snapshots']
            if any(type(s['batch']) is not int for s in snapshots) or [(s['phase'], s['batch']) for s in snapshots] != expected:
                raise ValueError('missing, duplicate or out-of-order memory phase')
            for s in snapshots:
                if any(type(s.get(k)) is not int or s[k] <= 0 for k in ['rss_bytes', 'process_high_water_bytes']) or s['process_high_water_bytes'] < s['rss_bytes']:
                    raise ValueError('invalid memory measurement')
                if s['phase'] in ['baseline', 'dropped']:
                    if s.get('engine_stats') is not None: raise ValueError('engine statistics outside engine lifetime')
                else:
                    names = {'heap_tracked','heap_collected_total','shapes_created'} if key[0] == 'napi-vm' else {'malloc_size','memory_used_size','obj_count','shape_count','atom_count'}
                    stats = s.get('engine_stats', {})
                    if set(stats) != names or any(type(v) is not int or v < 0 for v in stats.values()):
                        raise ValueError('invalid engine-specific memory statistics')
                if s['phase'] == 'collected':
                    if type(s.get('gc_ns')) is not int or s['gc_ns'] < 0: raise ValueError('invalid GC duration')
                elif s.get('gc_ns') is not None: raise ValueError('GC duration outside collection phase')
        actual = summary(row['runs'])
        if set(actual) != set(row['summary']): raise ValueError('incomplete memory summary')
        for name, values in actual.items():
            for field, value in values.items():
                stored = row['summary'][name].get(field)
                if type(stored) not in (int, float) or not math.isfinite(stored) or not math.isclose(stored, value, rel_tol=1e-12):
                    raise ValueError('inconsistent memory summary')
        rows[key] = row
    if set(rows) != {(e,w) for e in ENGINES for w in WORKLOADS}:
        raise ValueError('incomplete engine/workload memory matrix')
    return rows

def run_memory(directory, destination, samples, iterations, objects):
    if platform.system() != 'Linux': raise RuntimeError('memory probes require Linux /proc')
    if destination.exists(): raise RuntimeError('output must be new')
    build = json.loads((directory/'build.json').read_text())
    executable = directory/'memory'
    if sha(executable) != build.get('memory_executable_sha256'):
        raise RuntimeError('memory executable digest mismatch; rebuild the comparison crate')
    helper_hash = sha(Path(__file__))
    load = os.getloadavg()
    rows = {(e,w): {'engine': e, 'workload': w, 'runs': []} for e in ENGINES for w in WORKLOADS}
    for sample in range(samples):
        for workload in WORKLOADS:
            for engine in ENGINES if sample % 2 == 0 else ENGINES[::-1]:
                args = [str(executable.resolve()), '--engine', engine, '--workload', workload,
                        '--iterations', str(iterations), '--objects', str(objects)]
                result = json.loads(subprocess.check_output(args, text=True))
                rows[(engine,workload)]['runs'].append(result)
    for row in rows.values(): row['summary'] = summary(row['runs'])
    report = {'schema':1, 'kind':'process_memory', 'samples':samples, 'iterations':iterations,
              'objects_per_call':objects, 'results':list(rows.values()),
              'metadata': {'build':build, 'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
                           'run_helper_sha256':helper_hash, 'platform':platform.platform(),
                           'machine':platform.machine(), 'cpu_count':os.cpu_count(),
                           'cpu':next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown'),
                           'load_average_start':load, 'load_average_end':os.getloadavg(),
                           'limitations': ['One engine/workload per fresh process; same binary and shared guest source.',
                                           'Linux /proc RSS and VmHWM are approximate counters; reported VmHWM can decrease. Peaks use the maximum of all observed readings.',
                                           'RSS includes allocator arenas, resident code/data and shared pages, builtins, caches, and probe allocations.',
                                           'RSS after GC/drop does not directly count live objects or prove a leak; pages may remain in the allocator.',
                                           'Builtin sets differ; reports describe the configured engines rather than matched language completeness.',
                                           'GC durations measure explicit collection calls; automatic collection may also occur during allocations.',
                                           'napi-vm destruction is followed by a temporary collector; rquickjs drops context/runtime.',
                                           'Engine-specific statistics have different definitions: tracked containers/minted shapes versus QuickJS allocation/object/shape counts; do not compare these counters as identical metrics.',
                                           'No CPU isolation; descriptive process repetitions, not a significance test.']}}
    validate(report)
    if sha(Path(__file__)) != helper_hash: raise RuntimeError('memory report helper changed during run')
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, indent=2)+'\n')
    print(destination)


def compare(before, after):
    old, new = [json.loads(path.read_text()) for path in [before, after]]
    left, right = [validate(report) for report in [old, new]]
    for key in ['samples','iterations','objects_per_call']:
        if old[key] != new[key]: raise ValueError('incompatible memory settings: '+key)
    for key in ['platform','machine','cpu','cpu_count','run_helper_sha256']:
        if old['metadata'][key] != new['metadata'][key]: raise ValueError('incompatible memory host or report helper: '+key)
    builds = [r['metadata']['build'] for r in [old, new]]
    for key in ['profile','rustc','build_environment','comparison_dependencies']:
        if builds[0][key] != builds[1][key]: raise ValueError('incompatible memory build: '+key)
    fixed = [{k:v for k,v in b['inputs']['files'].items() if not k.startswith('crates/napi-vm-core/src/')} for b in builds]
    required = {'bench/rquickjs/src/bin/memory.rs','bench/rquickjs/Cargo.toml','bench/rquickjs/Cargo.lock','.cargo/config.toml'}
    if not required <= fixed[0].keys() or fixed[0] != fixed[1]:
        raise ValueError('incompatible memory harness/dependencies/configuration')
    results = []
    for key in sorted(left):
        a,b = left[key],right[key]
        if a['runs'][0]['source_sha256'] != b['runs'][0]['source_sha256']: raise ValueError('changed memory workload source')
        results.append({'engine':key[0],'workload':key[1], 'metrics':{
            name:{'before':a['summary'][name]['median'],'after':b['summary'][name]['median'],
                  'delta':b['summary'][name]['median']-a['summary'][name]['median']}
            for name in a['summary']}})
    return {'schema':1,'results':results,'warning':'RSS includes allocator/cache retention. Differences do not alone establish live-object leaks; GC durations are descriptive.'}

if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description='Validate and compare isolated memory reports.')
    parser.add_argument('before',type=Path);parser.add_argument('after',type=Path)
    args = parser.parse_args()
    print(json.dumps(compare(args.before,args.after),indent=2))
