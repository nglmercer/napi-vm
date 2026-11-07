#!/usr/bin/env python3
"""Build immutable loopback HTTP probes and retain checked transport timings."""
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

ROOT = Path(__file__).resolve().parent.parent

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def inputs():
    paths = [ROOT / name for name in ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'bench/http.py']]
    paths += [p for folder in ['crates/napi-vm-core', 'crates/napi-vm-runtime'] for p in (ROOT / folder).rglob('*')
              if p.is_file() and 'target' not in p.parts and p.suffix != '.md']
    return {str(p.relative_to(ROOT)): sha(p) for p in sorted(paths)}

def command(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, **kwargs)

def output(args):
    return command(args, stdout=subprocess.PIPE).stdout.strip()

def build(directory):
    directory.mkdir(parents=True, exist_ok=False)
    start = inputs()
    args = ['cargo', 'build', '--locked', '--profile', 'ci-optimized', '-p', 'napi-vm-runtime',
            '--features', 'runtime-net', '--example', 'http_bench']
    with (directory / 'build.log').open('w') as log:
        command(args, stdout=log, stderr=subprocess.STDOUT)
    if inputs() != start:
        raise RuntimeError('HTTP build inputs changed during compilation')
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    shutil.copy2(target / 'ci-optimized/examples/http_bench', directory / 'http-bench')
    record = {'inputs': start, 'executable_sha256': sha(directory / 'http-bench'),
              'command': args, 'profile': 'ci-optimized', 'features': 'runtime-net',
              'rustc': output(['rustc', '--version', '--verbose']),
              'git_head': output(['git', 'rev-parse', 'HEAD']), 'git_status': output(['git', 'status', '--short']),
              'build_environment': {k:v for k,v in os.environ.items() if k in ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTUP_TOOLCHAIN', 'CARGO_TARGET_DIR'] or k.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_'))}}
    (directory / 'build.json').write_text(json.dumps(record, indent=2)+'\n')
    print(directory)

def run(directory, destination, samples, iterations):
    record = json.loads((directory / 'build.json').read_text())
    executable = directory / 'http-bench'
    if sha(executable) != record['executable_sha256']:
        raise RuntimeError('HTTP executable digest mismatch')
    if destination.exists():
        raise RuntimeError('output must be new')
    load = os.getloadavg() if hasattr(os, 'getloadavg') else None
    result = json.loads(output([str(executable.resolve()), '--samples', str(samples), '--iterations', str(iterations)]))
    assert result['samples'] == samples and result['iterations'] == iterations and len(result['results']) == 4
    for row in result['results']:
        values = row['samples']
        median = statistics.median(values)
        row['summary'] = {'median': median, 'mad': statistics.median(abs(x-median) for x in values)}
        row['backend'] = 'production HTTP worker; loopback HTTP/1.1 Connection: close'
    result['metadata'] = {'build': record, 'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
                          'platform': platform.platform(), 'machine': platform.machine(),
                          'cpu': next((line.split(':', 1)[1].strip() for line in Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown') if Path('/proc/cpuinfo').exists() else platform.processor(),
                          'cpu_count': os.cpu_count(), 'load_average_start': load,
                          'load_average_end': os.getloadavg() if hasattr(os, 'getloadavg') else None,
                          'limitations': ['Input allocation and response checks are outside per-request timers.',
                                          'Includes runtime/client construction, transport, redirects, base64 and JSON response creation.',
                                          'Loopback HTTP server closes connections; no TLS, DNS, pooling, guest VM or external network measurement.',
                                          'No CPU isolation; repeat interleaved runs before accepting gains.']}
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(result, indent=2)+'\n')
    print(destination)

def read_report(path):
    report = json.loads(path.read_text())
    if report.get('schema') != 1 or type(report.get('samples')) is not int or report['samples'] <= 0 or report['samples'] % 2 != 1 or type(report.get('iterations')) is not int or report['iterations'] <= 0:
        raise ValueError('invalid HTTP report settings')
    rows = {}
    for row in report['results']:
        values = row['samples']
        if row['name'] in rows or row['unit'] != 'ns/request' or len(values) != report['samples'] or any(type(x) not in (int, float) or not math.isfinite(x) or x <= 0 for x in values):
            raise ValueError('invalid HTTP timing row')
        median = statistics.median(values)
        for key, actual in [('median', median), ('mad', statistics.median(abs(x-median) for x in values))]:
            stored = row['summary'].get(key)
            if type(stored) not in (int, float) or not math.isfinite(stored) or not math.isclose(stored, actual, rel_tol=1e-12):
                raise ValueError('inconsistent HTTP timing statistics')
        rows[row['name']] = row
    if set(rows) != {'get_empty', 'get_64k', 'post_4m', 'post_4m_redirect_3'}:
        raise ValueError('incomplete HTTP report')
    return report, rows

def compare(before, after):
    a, left = read_report(before)
    b, right = read_report(after)
    for key in ['samples', 'iterations']:
        if a[key] != b[key]: raise ValueError('incompatible HTTP settings: '+key)
    for key in ['platform', 'machine', 'cpu', 'cpu_count']:
        if a['metadata'][key] != b['metadata'][key]: raise ValueError('incompatible HTTP host: '+key)
    builds = [r['metadata']['build'] for r in [a, b]]
    for key in ['profile', 'features', 'rustc', 'build_environment']:
        if builds[0][key] != builds[1][key]: raise ValueError('incompatible HTTP build: '+key)
    fixed = [{k:v for k,v in build['inputs'].items() if not k.startswith('crates/napi-vm-runtime/src/')} for build in builds]
    required = {'Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'crates/napi-vm-runtime/Cargo.toml', 'crates/napi-vm-runtime/examples/http_bench.rs', 'crates/napi-vm-runtime/examples/http_bench/native.rs'}
    if not required <= fixed[0].keys() or fixed[0] != fixed[1]:
        raise ValueError('incompatible HTTP harness, dependencies or configuration')
    results = []
    for name in sorted(left):
        x, y = left[name], right[name]
        if x['backend'] != y['backend']: raise ValueError('incompatible HTTP boundary')
        results.append({'name': name, 'before_ns': x['summary']['median'], 'after_ns': y['summary']['median'], 'speedup': x['summary']['median']/y['summary']['median']})
    return {'schema': 1, 'results': results, 'changed_build_inputs': sorted(k for k in builds[0]['inputs'].keys() | builds[1]['inputs'].keys() if builds[0]['inputs'].get(k) != builds[1]['inputs'].get(k)), 'warning': 'Descriptive ratios only; inspect raw samples/MAD and repeat interleaved measurements.'}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    b = sub.add_parser('build'); b.add_argument('--output', required=True, type=Path)
    r = sub.add_parser('run'); r.add_argument('--build', required=True, type=Path); r.add_argument('--output', required=True, type=Path)
    r.add_argument('--samples', type=int, default=9); r.add_argument('--iterations', type=int, default=5)
    c = sub.add_parser('compare'); c.add_argument('before', type=Path); c.add_argument('after', type=Path)
    args = parser.parse_args()
    if args.command == 'build': build(args.output.resolve())
    elif args.command == 'compare': print(json.dumps(compare(args.before, args.after), indent=2))
    else:
        if args.samples <= 0 or args.samples % 2 != 1 or args.iterations <= 0:
            parser.error('samples must be positive and odd; iterations must be positive')
        run(args.build.resolve(), args.output.resolve(), args.samples, args.iterations)

if __name__ == '__main__':
    main()
