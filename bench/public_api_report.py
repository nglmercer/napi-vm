#!/usr/bin/env python3
"""Validate or compare persistent Node API timing reports."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics

CASES = tuple('node_api/'+name for name in (
    'construct_dispose', 'run_cached_source', 'get_global_scalar', 'has_global',
    'set_global_scalar', 'remove_global', 'call_no_arguments', 'call_eight_numbers',
    'call_plain_control', 'call_host_callback', 'replace_host_function',
    'set_object_1', 'return_object_1', 'set_object_128', 'return_object_128',
    'set_bytes_1024', 'return_bytes_1024', 'set_bytes_65536', 'return_bytes_65536',
    'validate_module', 'define_module', 'register_module', 'has_module',
    'remove_module', 'list_modules', 'collect_cycles_idle'))
SETTINGS = ('scriptSha256', 'node', 'platform', 'arch', 'cpu', 'samples', 'iterations')


def read_report(path):
    report = json.loads(path.read_text())
    if report.get('schema') != 1 or not isinstance(report.get('metadata'), dict):
        raise ValueError('invalid public API schema')
    meta = report['metadata']
    for key in ('samples', 'iterations'):
        if type(meta.get(key)) is not int or meta[key] <= 0:
            raise ValueError('invalid public API ' + key)
    if meta['samples'] % 2 != 1:
        raise ValueError('public API sample count must be odd')
    for key in ('addonSha256', 'scriptSha256'):
        if not isinstance(meta.get(key), str) or not re.fullmatch('[0-9a-f]{64}', meta[key]):
            raise ValueError('invalid public API ' + key)
    for key in ('node', 'platform', 'arch', 'cpu'):
        if not isinstance(meta.get(key), str) or not meta[key]:
            raise ValueError('missing public API ' + key)
    rows = {}
    if not isinstance(report.get('results'), list):
        raise ValueError('missing public API results')
    for row in report['results']:
        name = row.get('name')
        if name not in CASES or name in rows or row.get('unit') != 'ns/call' or not isinstance(row.get('backend'), str) or not row['backend']:
            raise ValueError('invalid or duplicate public API boundary')
        values = row.get('samples')
        if not isinstance(values, list) or len(values) != meta['samples'] or any(type(v) not in (int, float) or not math.isfinite(v) or v <= 0 for v in values):
            raise ValueError('invalid public API timing samples')
        median = statistics.median(values)
        for key, value in {'median': median, 'mad': statistics.median(abs(v-median) for v in values)}.items():
            stored = row.get('summary', {}).get(key)
            if type(stored) not in (int, float) or not math.isfinite(stored) or not math.isclose(stored, value, rel_tol=1e-12):
                raise ValueError('invalid public API ' + key)
        rows[name] = row
    if set(rows) != set(CASES):
        raise ValueError('incomplete public API matrix')
    return report, rows


def compare(before, after):
    old, left = read_report(before)
    new, right = read_report(after)
    if any(old['metadata'][key] != new['metadata'][key] for key in SETTINGS):
        raise ValueError('incompatible public API settings or host')
    results = []
    for name in CASES:
        x, y = left[name], right[name]
        if x['backend'] != y['backend']:
            raise ValueError('changed public API timing boundary')
        results.append({'name': name, 'before_median_ns': x['summary']['median'],
            'after_median_ns': y['summary']['median'], 'before_mad_ns': x['summary']['mad'],
            'after_mad_ns': y['summary']['mad'], 'speedup': x['summary']['median']/y['summary']['median']})
    return {'schema': 1, 'before_addon_sha256': old['metadata']['addonSha256'],
            'after_addon_sha256': new['metadata']['addonSha256'], 'results': results,
            'limitations': ['Descriptive ratios under host noise; not a significance test.',
                            'Inspect build manifests separately: the validator checks host settings and timing boundaries.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(compare(args.before, args.after), indent=2))
    except (ValueError, OSError) as error:
        parser.exit(1, str(error)+'\n')
