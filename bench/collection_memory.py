#!/usr/bin/env python3
"""Measure native Node-owner Map/Set memory in separate processes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import statistics
import subprocess

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT/'bench/performance/collection-memory.cjs'
CASES = {f'{kind.lower()}_{size}': (kind, count, entries)
         for kind in ('Map', 'Set') for size, count, entries in (('small', 1000, 8), ('large', 4, 4096))}
PHASES = [('baseline', 0), ('ready', 0)] + [(phase, n) for n in range(1, 4) for phase in ('allocated', 'collected')] + [('released', 3), ('disposed', 3)]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def metrics(run):
    phases = {s['phase']: s for s in run['snapshots']}
    return {'ready_rss_bytes': phases['ready']['rss_bytes'],
            'last_collected_rss_bytes': phases['collected']['rss_bytes'],
            'collected_growth_bytes': phases['collected']['rss_bytes']-phases['ready']['rss_bytes'],
            'released_rss_bytes': phases['released']['rss_bytes'],
            'disposed_rss_bytes': phases['disposed']['rss_bytes'],
            'peak_rss_bytes': max(s['high_water_bytes'] for s in run['snapshots']),
            'ready_tracked': phases['ready']['engine_stats']['tracked'],
            'released_tracked': phases['released']['engine_stats']['tracked']}


def summarize(runs):
    rows = [metrics(run) for run in runs]
    result = {}
    for key in rows[0]:
        values = [row[key] for row in rows]
        median = statistics.median(values)
        result[key] = {'median': median, 'mad': statistics.median(abs(v-median) for v in values)}
    return result


def validate(report):
    if report.get('schema') != 1 or report.get('kind') != 'node_collection_memory':
        raise ValueError('invalid owner memory schema')
    samples = report.get('samples')
    if type(samples) is not int or samples <= 0 or samples % 2 != 1:
        raise ValueError('owner memory samples must be positive and odd')
    meta = report.get('metadata', {})
    for key in ('addon_sha256', 'script_sha256', 'helper_sha256'):
        if not isinstance(meta.get(key), str) or not re.fullmatch('[0-9a-f]{64}', meta[key]):
            raise ValueError('invalid owner memory '+key)
    for key in ('node', 'platform', 'machine', 'cpu'):
        if not isinstance(meta.get(key), str) or not meta[key]:
            raise ValueError('missing owner memory '+key)
    rows = {}
    for row in report.get('results', []):
        name = row.get('name')
        if name not in CASES or name in rows or len(row.get('runs', [])) != samples:
            raise ValueError('invalid or duplicate owner memory case')
        kind, count, entries = CASES[name]
        sources = set()
        for run in row['runs']:
            if (run.get('schema'), run.get('kind'), run.get('collections'), run.get('entries'), run.get('rounds')) != (1, kind, count, entries, 3):
                raise ValueError('invalid owner memory dimensions')
            if run.get('addon_sha256') != report['metadata']['addon_sha256'] or run.get('script_sha256') != report['metadata']['script_sha256']:
                raise ValueError('owner memory binary or harness mismatch')
            if not isinstance(run.get('source_sha256'), str) or len(run['source_sha256']) != 64:
                raise ValueError('missing owner memory source hash')
            sources.add(run['source_sha256'])
            snaps = run.get('snapshots', [])
            if [(s.get('phase'), s.get('round')) for s in snaps] != PHASES:
                raise ValueError('incomplete owner memory phase sequence')
            for snap in snaps:
                if any(type(snap.get(k)) is not int or snap[k] <= 0 for k in ('rss_bytes', 'high_water_bytes')):
                    raise ValueError('invalid owner memory bytes')
                stats = snap.get('engine_stats')
                if snap['phase'] in ('baseline', 'disposed'):
                    if stats is not None:
                        raise ValueError('unexpected owner stats without an owner')
                elif not isinstance(stats, dict) or any(type(stats.get(k)) is not int or stats[k] < 0 for k in ('tracked', 'collectedTotal')):
                    raise ValueError('invalid owner heap counters')
        if len(sources) != 1 or row.get('summary') != summarize(row['runs']):
            raise ValueError('inconsistent owner source or stored summary')
        rows[name] = row
    if set(rows) != set(CASES):
        raise ValueError('incomplete owner memory matrix')
    return rows


def run(addon, destination, samples):
    addon = addon.resolve()
    if destination.exists():
        raise ValueError('output must be new')
    report = {'schema': 1, 'kind': 'node_collection_memory', 'samples': samples,
              'metadata': {'addon_path': str(addon), 'addon_sha256': sha(addon), 'script_sha256': sha(SCRIPT),
                           'helper_sha256': sha(Path(__file__)),
                           'node': subprocess.check_output(['node','--version'], text=True).strip(),
                           'platform': platform.platform(), 'machine': platform.machine(),
                           'cpu': next((line.split(':',1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown')},
              'limitations': ['Linux process RSS includes Node, builtins, native/V8 allocator retention and probe buffers.',
                              'No host/CPU isolation; medians/MAD are descriptive; small differences may be noise.',
                              'VmRSS and VmHWM are approximate. Use maximum observed high-water reading, without assuming monotonicity.',
                              'Separate owner per process; three replacement rounds, explicit collection, root release, then disposal.',
                              'Heap counters are native tracked containers, not a complete allocation census or a rquickjs object count.'],
              'results': []}
    for name, (kind, count, entries) in CASES.items():
        runs = []
        for _ in range(samples):
            raw = subprocess.check_output(['node',str(SCRIPT),kind,str(count),str(entries)],
                    env={**os.environ,'NAPI_BENCH_BINDING':str(addon)}, text=True)
            runs.append(json.loads(raw))
        report['results'].append({'name': name, 'runs': runs, 'summary': summarize(runs)})
    validate(report)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, indent=2)+'\n')


def compare(before, after):
    old, new = [json.loads(path.read_text()) for path in (before, after)]
    left, right = validate(old), validate(new)
    for key in ('script_sha256', 'helper_sha256', 'node', 'platform', 'machine', 'cpu'):
        if old['metadata'][key] != new['metadata'][key]:
            raise ValueError('incompatible owner memory '+key)
    if old['samples'] != new['samples']:
        raise ValueError('incompatible owner memory sample count')
    result = []
    for name in CASES:
        if left[name]['runs'][0]['source_sha256'] != right[name]['runs'][0]['source_sha256']:
            raise ValueError('changed owner memory workload')
        result.append({'name': name, 'before': left[name]['summary'], 'after': right[name]['summary']})
    return {'schema': 1, 'results': result, 'limitations': new['limitations'],
            'before_addon_sha256': old['metadata']['addon_sha256'], 'after_addon_sha256': new['metadata']['addon_sha256']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    r = sub.add_parser('run')
    r.add_argument('--addon', type=Path, required=True)
    r.add_argument('--output', type=Path, required=True)
    r.add_argument('--samples', type=int, default=3)
    c = sub.add_parser('compare')
    c.add_argument('before', type=Path)
    c.add_argument('after', type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'run':
            if args.samples <= 0 or args.samples % 2 != 1:
                raise ValueError('samples must be positive and odd')
            run(args.addon, args.output, args.samples)
        else:
            print(json.dumps(compare(args.before,args.after), indent=2))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, str(error)+'\n')
