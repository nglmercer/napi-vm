#!/usr/bin/env python3
"""Validate and compare outbound Map/Set bridge measurements."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics

CASES = {f'{kind}_{refs}_references': refs for kind in ('map', 'set') for refs in (1, 64)}
SETTINGS = ('scriptSha256', 'node', 'platform', 'arch', 'cpu', 'samples', 'iterations')


def read_report(path):
    report = json.loads(path.read_text())
    if report.get('schema') != 1 or not isinstance(report.get('metadata'), dict):
        raise ValueError('invalid collection report schema')
    meta = report['metadata']
    for key in ('samples', 'iterations'):
        if type(meta.get(key)) is not int or meta[key] <= 0:
            raise ValueError('invalid ' + key)
    if meta['samples'] % 2 != 1:
        raise ValueError('sample count must be odd')
    for key in ('addonSha256', 'scriptSha256'):
        if not isinstance(meta.get(key), str) or not re.fullmatch('[0-9a-f]{64}', meta[key]):
            raise ValueError('invalid ' + key)
    for key in ('node', 'platform', 'arch', 'cpu'):
        if not isinstance(meta.get(key), str) or not meta[key]:
            raise ValueError('missing ' + key)
    rows = {}
    if not isinstance(report.get('results'), list):
        raise ValueError('missing collection results')
    for row in report['results']:
        name = row.get('name')
        if name not in CASES or name in rows:
            raise ValueError('unknown or duplicate collection case')
        if row.get('unit') != 'ns/call' or row.get('references') != CASES[name] or row.get('expectedEntries') != 256:
            raise ValueError('invalid collection boundary or dimensions')
        if not isinstance(row.get('source'), str) or not row['source']:
            raise ValueError('missing guest source')
        if type(row.get('sharedIdentityPreserved')) is not bool or (CASES[name] == 1 and not row['sharedIdentityPreserved']):
            raise ValueError('invalid identity observation')
        values = row.get('samples')
        if not isinstance(values, list) or len(values) != meta['samples'] or any(type(v) not in (int, float) or not math.isfinite(v) or v <= 0 for v in values):
            raise ValueError('invalid collection timing samples')
        median = statistics.median(values)
        expected = {'median': median, 'mad': statistics.median(abs(v-median) for v in values)}
        for key, value in expected.items():
            stored = row.get('summary', {}).get(key)
            if type(stored) not in (int, float) or not math.isfinite(stored) or not math.isclose(stored, value, rel_tol=1e-12):
                raise ValueError('invalid collection ' + key)
        rows[name] = row
    if set(rows) != set(CASES):
        raise ValueError('incomplete collection matrix')
    return report, rows


def compare(before, after):
    old, left = read_report(before)
    new, right = read_report(after)
    changed = [key for key in SETTINGS if old['metadata'][key] != new['metadata'][key]]
    if changed:
        raise ValueError('incompatible collection settings: ' + ', '.join(changed))
    results = []
    for name in CASES:
        x, y = left[name], right[name]
        if x['source'] != y['source']:
            raise ValueError('changed guest source: ' + name)
        results.append({'name': name, 'unit': x['unit'],
                        'before_median_ns': x['summary']['median'], 'after_median_ns': y['summary']['median'],
                        'before_mad_ns': x['summary']['mad'], 'after_mad_ns': y['summary']['mad'],
                        'speedup': x['summary']['median']/y['summary']['median'],
                        'identity_before': x['sharedIdentityPreserved'], 'identity_after': y['sharedIdentityPreserved']})
    return {'schema': 1, 'before_addon_sha256': old['metadata']['addonSha256'],
            'after_addon_sha256': new['metadata']['addonSha256'], 'results': results,
            'limitations': ['Descriptive ratios; no CPU isolation or significance test.',
                            'Identity observations are explicit: a defective before result is not equivalent behavior.',
                            'Compare build provenance separately; the report validates timing boundaries and host settings.']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('before', type=Path)
    parser.add_argument('after', type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(compare(args.before, args.after), indent=2))
    except (ValueError, OSError) as error:
        parser.exit(1, str(error) + '\n')
