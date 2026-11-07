#!/usr/bin/env python3
"""Counterbalance immutable engine builds on one Linux CPU."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import resource
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parent.parent
SPEC=importlib.util.spec_from_file_location('engine_compare',ROOT/'bench/rquickjs-compare.py')
COMPARE=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(COMPARE)
CASES=('property_reads','class_methods','array_index_updates')


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--before',type=Path,required=True)
    p.add_argument('--after',type=Path,required=True)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--cpu',type=int)
    p.add_argument('--samples',type=int,default=31)
    p.add_argument('--iterations',type=int,default=25)
    args=p.parse_args()
    if not hasattr(os,'sched_setaffinity'):
        p.error('this supplemental driver requires Linux CPU affinity')
    allowed=sorted(os.sched_getaffinity(0))
    cpu=args.cpu if args.cpu is not None else allowed[-1]
    if cpu not in allowed or args.samples<=0 or args.samples%2!=1 or args.iterations<=0:
        p.error('CPU must be allowed; samples positive and odd; iterations positive')
    out=args.output.resolve();out.mkdir(parents=True,exist_ok=False)
    builds={'before':args.before.resolve(),'after':args.after.resolve()}
    helper_sha=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    result={'schema':1,'status':'running','cpu_affinity':[cpu],'allowed_cpus':allowed,
            'helper_sha256':helper_sha,'samples':args.samples,'iterations':args.iterations,
            'comparisons':[],'limitations':['Affinity prevents cross-CPU migrations, not competition from other work.',
                'Both engines alternate within each process; build order reverses in the second pair.',
                'Wall-time timings, process CPU/wall counters and raw samples are descriptive, not significance tests.',
                'Small ratios can still change with frequency, caches, code placement and host activity.']}
    def save():
        (out/'controls.json').write_text(json.dumps(result,indent=2)+'\n')
    save()
    try:
        with (out/'run.log').open('w') as log:
            for case in CASES:
                for pair in (1,2):
                    for stage in (('before','after') if pair==1 else ('after','before')):
                        path=out/f'{case}-{stage}-{pair}.json'
                        command=[sys.executable,'bench/rquickjs.py','run','--build',str(builds[stage]),
                            '--output',str(path),'--samples',str(args.samples),'--iterations',str(args.iterations),'--filter',case]
                        usage_before=resource.getrusage(resource.RUSAGE_CHILDREN)
                        start=time.monotonic_ns()
                        subprocess.run(command,cwd=ROOT,check=True,stdout=log,stderr=subprocess.STDOUT,
                            preexec_fn=lambda:os.sched_setaffinity(0,{cpu}))
                        elapsed=time.monotonic_ns()-start
                        usage_after=resource.getrusage(resource.RUSAGE_CHILDREN)
                        report=json.loads(path.read_text())
                        report['metadata']['supplemental_control']={
                            'cpu_affinity':[cpu],'helper_sha256':helper_sha,
                            'wrapper_command':command,'wrapper_wall_ns':elapsed,
                            'process_tree_cpu_seconds':usage_after.ru_utime+usage_after.ru_stime-usage_before.ru_utime-usage_before.ru_stime,
                            'involuntary_context_switches':usage_after.ru_nivcsw-usage_before.ru_nivcsw}
                        limitations=report['metadata']['limitations']
                        report['metadata']['limitations']=[line for line in limitations if not line.startswith('No host isolation or enforced CPU affinity')]
                        report['metadata']['limitations'].append(f'Supplemental CPU affinity restricted this wrapper and its benchmark child to CPU {cpu}; host activity is not isolated.')
                        path.write_text(json.dumps(report,indent=2)+'\n')
                    before=out/f'{case}-before-{pair}.json';after=out/f'{case}-after-{pair}.json'
                    compared=COMPARE.changes(before,after)
                    result['comparisons'].append({'case':case,'pair':pair,'results':compared['results']})
                    save()
                print(case+' controls complete',flush=True)
        result['status']='complete'
    except Exception as error:
        result.update(status='failed',error=str(error))
        raise
    finally:
        save()
    print(out/'controls.json')


if __name__=='__main__':main()
