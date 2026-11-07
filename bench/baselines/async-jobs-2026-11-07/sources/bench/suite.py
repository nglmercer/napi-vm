#!/usr/bin/env python3
"""Run the layered performance suite and retain an explicit coverage manifest."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
GROUPS = ('engines', 'memory', 'http', 'host', 'platforms', 'async')

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--groups', default=','.join(GROUPS))
    parser.add_argument('--samples', type=int, default=15)
    parser.add_argument('--iterations', type=int, default=10)
    parser.add_argument('--memory-samples', type=int, default=3)
    parser.add_argument('--memory-iterations', type=int, default=8)
    parser.add_argument('--memory-objects', type=int, default=2000)
    parser.add_argument('--platform-processes', type=int, default=3)
    parser.add_argument('--async-processes', type=int, default=3)
    parser.add_argument('--tmp-dir', type=Path)
    args = parser.parse_args()
    requested = args.groups.split(',')
    if not requested or len(set(requested)) != len(requested) or any(g not in GROUPS for g in requested):
        parser.error('groups must be a nonempty, unique selection of '+','.join(GROUPS))
    if any(v <= 0 for v in [args.samples,args.iterations,args.memory_samples,args.memory_iterations,args.memory_objects,args.platform_processes,args.async_processes]) or args.samples % 2 != 1 or args.memory_samples % 2 != 1 or args.platform_processes % 2 != 1 or args.async_processes % 2 != 1:
        parser.error('sample counts must be positive and odd; iterations/objects must be positive')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    if args.tmp_dir:
        temp = args.tmp_dir.resolve(); temp.mkdir(parents=True, exist_ok=True)
        env['TMPDIR'] = str(temp)
    manifest = {'schema':1, 'status':'running', 'all_groups_complete':False,
                'selected_groups':requested, 'unselected_groups':[g for g in GROUPS if g not in requested],
                'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
                'driver_sha256':sha(Path(__file__)), 'steps':[], 'reports':[],
                'limitations':['Finite representative operation coverage; see docs/performance.md for unmeasured families.',
                               'Each layer has its own units and timed boundary; no aggregate runtime score is computed.',
                               'Drivers run sequentially, so suite compilation does not overlap suite measurements.',
                               'External host activity is not isolated; retain samples/MAD and repeat comparisons.']}
    def save():
        (out/'suite.json').write_text(json.dumps(manifest, indent=2)+'\n')
    def step(name, arguments):
        record = {'name':name, 'command':arguments, 'status':'running'}
        manifest['steps'].append(record); save()
        with (out/(name+'.log')).open('w') as log:
            result = subprocess.run(arguments, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        record.update(status='passed' if result.returncode == 0 else 'failed', exit_code=result.returncode)
        save()
        if result.returncode: raise RuntimeError(f'{name} failed; inspect {out/(name+".log")}')
    def report(group, path):
        data = json.loads(path.read_text())
        if data.get('schema') != 1 or not data.get('results'):
            raise ValueError(f'invalid or empty {group} report: {path}')
        manifest['reports'].append({'group':group,'path':str(path.relative_to(out)),
                                    'sha256':sha(path),'result_rows':len(data['results'])})
        save()
    py = sys.executable
    save()
    try:
        # Build shared artifacts once; every timed command uses a verified copy.
        if set(requested) & {'engines','memory','platforms'}:
            step('engine-build',[py,'bench/rquickjs.py','build','--output',str(out/'engine-build')])
        if 'http' in requested:
            step('http-build',[py,'bench/http.py','build','--output',str(out/'http-build')])
        if 'platforms' in requested:
            step('platform-build',[py,'bench/platforms.py','build','--engine-build',str(out/'engine-build'),
                                   '--output',str(out/'platform-build')])
        if 'async' in requested:
            step('async-build',[py,'bench/async_jobs.py','build','--output',str(out/'async-build')])
        # The broad host runner has its own sequential build-and-measure phases.
        if 'host' in requested:
            step('host',[py,'bench/performance.py','run','--output',str(out/'host')])
            report('host',out/'host/baseline.json')
        if 'engines' in requested:
            path = out/'engines.json'
            step('engines',[py,'bench/rquickjs.py','run','--build',str(out/'engine-build'),
                            '--output',str(path),'--samples',str(args.samples),'--iterations',str(args.iterations)])
            step('engine-ratios',[py,'bench/rquickjs-compare.py','engines',str(path)])
            report('engines',path)
        if 'memory' in requested:
            path = out/'memory.json'
            step('memory',[py,'bench/rquickjs.py','memory','--build',str(out/'engine-build'),
                           '--output',str(path),'--samples',str(args.memory_samples),
                           '--iterations',str(args.memory_iterations),'--objects',str(args.memory_objects)])
            # Self-comparison validates the full raw matrix and stored summaries.
            step('memory-validate',[py,'bench/memory_report.py',str(path),str(path)])
            report('memory',path)
        if 'http' in requested:
            path = out/'http.json'
            step('http',[py,'bench/http.py','run','--build',str(out/'http-build'),
                         '--output',str(path),'--samples',str(args.samples),'--iterations',str(args.iterations)])
            step('http-validate',[py,'bench/http.py','compare',str(path),str(path)])
            report('http',path)
        if 'platforms' in requested:
            path=out/'platforms.json'
            step('platforms',[py,'bench/platforms.py','run','--build',str(out/'platform-build'),
                              '--output',str(path),'--samples',str(args.samples),'--iterations',str(args.iterations),
                              '--processes',str(args.platform_processes)])
            step('platform-validate',[py,'bench/platforms.py','compare',str(path),str(path)])
            report('platforms',path)
        if 'async' in requested:
            path=out/'async.json'
            step('async',[py,'bench/async_jobs.py','run','--build',str(out/'async-build'),
                          '--output',str(path),'--samples',str(args.samples),'--iterations',str(args.iterations),
                          '--processes',str(args.async_processes)])
            step('async-validate',[py,'bench/async_jobs.py','compare',str(path),str(path)])
            step('async-ratios',[py,'bench/async_jobs.py','ratios',str(path)])
            report('async',path)
        if set(r['group'] for r in manifest['reports']) != set(requested):
            raise RuntimeError('selected suite groups did not all produce reports')
        manifest.update(status='complete', all_groups_complete=set(requested)==set(GROUPS))
    except Exception as error:
        manifest.update(status='failed',error=str(error))
        raise
    finally:
        manifest['finished_utc'] = datetime.datetime.now(datetime.timezone.utc).isoformat(); save()
    print(out/'suite.json')

if __name__ == '__main__':
    main()
