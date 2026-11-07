#!/usr/bin/env python3
"""Describe per-program public-API ratios without combining startup components."""
import argparse
import hashlib
import json
from pathlib import Path
from platforms import ENGINES, MODES, validate


def ratios(path):
    data=json.loads(path.read_text());workloads=validate(data)
    rows={(r['engine'],r['mode'],r['workload']):r for r in data['results']}
    result=[]
    keys=[(mode,name) for name in workloads for mode in MODES]+[('construct_dispose','vm_lifecycle')]
    for mode,name in keys:
        medians={engine:rows[(engine,mode,name)]['median'] for engine in ENGINES}
        result.append({'workload':name,'mode':mode,'unit':rows[(ENGINES[0],mode,name)]['unit'],
            'medians':medians,'mads':{engine:rows[(engine,mode,name)]['mad'] for engine in ENGINES},
            'wasm_over_native':medians['napi-wasm']/medians['napi-native'],
            'native_over_node_context':medians['napi-native']/medians['node-v8'],
            'wasm_over_node_context':medians['napi-wasm']/medians['node-v8']})
    return {'schema':1,'source_report_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
            'results':result,'startup_components':[r for r in data['results'] if r['workload']=='startup'],
            'limitations':data['metadata']['limitations']+[
                'A ratio above one means the numerator took longer for this public API boundary.',
                'Component startup rows have different boundaries and are not combined into an engine score.',
                'Ratios of process medians are descriptive, not significance tests.']}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('report',type=Path);args=parser.parse_args()
    try:print(json.dumps(ratios(args.report),indent=2))
    except (ValueError,OSError) as error:parser.exit(1,str(error)+'\n')
