#!/usr/bin/env python3
"""Build and compare completed promise workloads and explicit job phases."""
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

ROOT = Path(__file__).resolve().parent.parent
MODES = ('enqueue', 'checkpoint', 'source_completed', 'call_completed')
ENGINES = ('napi-vm', 'rquickjs')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inputs():
    files = [p for folder in ('crates/napi-vm-core', 'bench/rquickjs')
             for p in (ROOT/folder).rglob('*') if p.is_file() and 'target' not in p.parts and p.suffix != '.md']
    files.extend((Path(__file__), ROOT/'.cargo/config.toml'))
    return {str(p.relative_to(ROOT)): sha(p) for p in sorted(set(files))}


def summary(values):
    if not values or any(type(v) not in (int, float) or not math.isfinite(v) or v <= 0 for v in values):
        raise ValueError('invalid async timing samples')
    mid = statistics.median(values)
    return {'median': mid, 'mad': statistics.median(abs(v-mid) for v in values)}


def fixtures(data):
    if data.get('schema') != 1 or data.get('kind') != 'async_workloads':
        raise ValueError('invalid async fixture export')
    result = {}
    for w in data.get('workloads', []):
        name = w.get('name')
        if not isinstance(name, str) or not name or name in result:
            raise ValueError('missing or duplicate async fixture')
        if not all(isinstance(w.get(k), str) and w[k] for k in ('source', 'setup', 'invocation')):
            raise ValueError('missing async source')
        if w['source'] != w['setup']+'\n'+w['invocation'] or hashlib.sha256(w['source'].encode()).hexdigest() != w.get('source_sha256'):
            raise ValueError('changed async source/hash')
        if type(w.get('expected')) not in (int, float) or not math.isfinite(w['expected']):
            raise ValueError('invalid async expected result')
        result[name] = w
    if not result:
        raise ValueError('empty async fixtures')
    return result


def fixture_digest(data):
    return hashlib.sha256(json.dumps(data,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def build(out):
    out = out.resolve(); out.mkdir(parents=True, exist_ok=False)
    before = inputs()
    command = ['cargo', 'build', '--locked', '--release', '--jobs', '2', '--manifest-path',
               str(ROOT/'bench/rquickjs/Cargo.toml'), '--target-dir', str(ROOT/'target/rquickjs-comparison'), '--bin', 'async_jobs']
    with (out/'build.log').open('w') as log:
        subprocess.run(command, cwd=ROOT, check=True, stdout=log, stderr=subprocess.STDOUT)
    if inputs() != before:
        raise RuntimeError('async compiler inputs changed during build')
    shutil.copy2(ROOT/'target/rquickjs-comparison/release/async_jobs', out/'async-jobs')
    raw = subprocess.check_output([str(out/'async-jobs'), '--emit-workloads'], text=True)
    fixtures(json.loads(raw)); (out/'workloads.json').write_text(raw)
    record = {'schema': 1, 'kind': 'async_build', 'inputs': before, 'command': command,
              'worker_sha256': sha(out/'async-jobs'), 'fixtures_sha256': sha(out/'workloads.json'),
              'driver_sha256': sha(Path(__file__)), 'fixture_matrix_sha256': fixture_digest(json.loads(raw)),
              'rustc': subprocess.check_output(['rustc', '--version', '--verbose'], text=True),
              'profile': 'release; opt-level=3; thin LTO; codegen-units=1; default core/rquickjs features',
              'environment': {k: v for k, v in os.environ.items() if k in ('RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTUP_TOOLCHAIN', 'CARGO_BUILD_TARGET', 'CARGO_ENCODED_RUSTFLAGS', 'CC', 'AR', 'CFLAGS', 'CXXFLAGS') or k.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_'))}}
    (out/'build.json').write_text(json.dumps(record, indent=2)+'\n'); print(out)


def validate_child(run, workloads, samples, iterations):
    if (run.get('schema'), run.get('kind'), run.get('samples'), run.get('iterations'), run.get('warmup_calls')) != (1, 'async_job_timings', samples, iterations, 5):
        raise ValueError('invalid async child settings')
    if any(type(run[k]) is not int for k in ('schema', 'samples', 'iterations', 'warmup_calls')):
        raise ValueError('non-integer async settings')
    if fixtures(dict(schema=1, kind='async_workloads', workloads=run.get('workloads'))) != workloads:
        raise ValueError('changed async child fixture matrix')
    rows = {}
    for r in run.get('results', []):
        key = tuple(r.get(k) for k in ('engine', 'mode', 'workload'))
        e, m, w = key
        if e not in ENGINES or m not in MODES or w not in workloads or key in rows:
            raise ValueError('unknown or duplicate async row')
        if r.get('unit') != 'ns/operation' or r.get('source_sha256') != workloads[w]['source_sha256']:
            raise ValueError('changed async boundary/source')
        if len(r.get('samples', [])) != samples or summary(r['samples']) != {k: r.get(k) for k in ('median', 'mad')}:
            raise ValueError('invalid async row statistics')
        if any(type(r.get(k)) not in (int, float) for k in ('median', 'mad')):
            raise ValueError('invalid async stored statistic types')
        rows[key] = r
    if len(rows) != len(ENGINES)*len(MODES)*len(workloads):
        raise ValueError('incomplete async matrix')
    counts = {}
    for r in run.get('job_counts', []):
        name = r.get('workload')
        if name not in workloads or name in counts or any(type(r.get(k)) is not int or r[k] <= 0 for k in ('napi_jobs', 'rquickjs_jobs')):
            raise ValueError('invalid async job diagnostic')
        counts[name] = r
    if counts.keys() != workloads.keys():
        raise ValueError('incomplete async job diagnostics')
    return rows


def aggregate(runs, workloads):
    rows = []
    for name, w in workloads.items():
        for mode in MODES:
            for engine in ENGINES:
                values = [next(r['median'] for r in run['results'] if (r['engine'], r['mode'], r['workload']) == (engine, mode, name)) for run in runs]
                rows.append(dict(engine=engine, mode=mode, workload=name, source_sha256=w['source_sha256'], unit='ns/operation', samples=values, **summary(values)))
    return rows


def validate(report):
    if report.get('schema') != 1 or report.get('kind') != 'async_jobs':
        raise ValueError('invalid async report')
    for k in ('samples', 'iterations', 'processes'):
        if type(report.get(k)) is not int or report[k] <= 0 or (k != 'iterations' and report[k] % 2 != 1):
            raise ValueError('invalid async report settings')
    workloads = fixtures(report['fixtures']); runs = report.get('runs', [])
    if len(runs) != report['processes']:
        raise ValueError('incomplete async process repetitions')
    meta = report['metadata']; b = meta['build']
    if any(not isinstance(b.get(k),str) or not re.fullmatch('[0-9a-f]{64}',b[k]) for k in ('worker_sha256','fixtures_sha256','driver_sha256','fixture_matrix_sha256')):
        raise ValueError('invalid async build digests')
    if fixture_digest(report['fixtures']) != b['fixture_matrix_sha256']:
        raise ValueError('async fixtures differ from build export')
    if b.get('kind') != 'async_build' or meta['driver_sha256'] != b.get('driver_sha256'):
        raise ValueError('async build/helper mismatch')
    if not all(isinstance(meta.get(k), str) and meta[k] for k in ('cpu', 'platform', 'machine')):
        raise ValueError('missing async host provenance')
    for run in runs:
        validate_child(run, workloads, report['samples'], report['iterations'])
        if run['job_counts'] != runs[0]['job_counts']:
            raise ValueError('async job counts changed between processes')
    if aggregate(runs, workloads) != report.get('results'):
        raise ValueError('invalid async process aggregation')
    return workloads


def run(directory, out, samples, iterations, processes):
    directory = directory.resolve()
    if out.exists():
        raise ValueError('async report output must be new')
    record = json.loads((directory/'build.json').read_text())
    if record['driver_sha256'] != sha(Path(__file__)) or sha(directory/'async-jobs') != record['worker_sha256'] or sha(directory/'workloads.json') != record['fixtures_sha256']:
        raise ValueError('async build artifacts/helper changed')
    data = json.loads((directory/'workloads.json').read_text()); workloads = fixtures(data)
    loads = []; runs = []
    for _ in range(processes):
        loads.append(os.getloadavg()); child = json.loads(subprocess.check_output([str(directory/'async-jobs'), '--samples', str(samples), '--iterations', str(iterations)], text=True, cwd=ROOT))
        validate_child(child, workloads, samples, iterations); runs.append(child)
    cpuinfo = Path('/proc/cpuinfo')
    report = dict(schema=1, kind='async_jobs', samples=samples, iterations=iterations, processes=processes, fixtures=data,
                  runs=runs, results=aggregate(runs, workloads), metadata=dict(build=record, driver_sha256=sha(Path(__file__)),
                      utc=datetime.datetime.now(datetime.timezone.utc).isoformat(), platform=platform.platform(), machine=platform.machine(),
                      cpu=next((l.split(':', 1)[1].strip() for l in cpuinfo.read_text().splitlines() if l.startswith('model name')), 'unknown') if cpuinfo.exists() else platform.processor() or 'unknown',
                      load_average_at_process_start=loads, cpu_count=os.cpu_count(), affinity=sorted(os.sched_getaffinity(0)) if hasattr(os,'sched_getaffinity') else None, limitations=runs[0]['limitations']+['Aggregate statistics are medians/MAD of independent process medians; all child batch samples are retained.']))
    validate(report); out.parent.mkdir(parents=True, exist_ok=True); out.write_text(json.dumps(report, indent=2)+'\n'); print(out)


def compare(a, b):
    old, new = (json.loads(p.read_text()) for p in (a, b))
    if validate(old) != validate(new):
        raise ValueError('async fixtures differ')
    for k in ('samples', 'iterations', 'processes'):
        if old[k] != new[k]: raise ValueError('async settings differ')
    for k in ('cpu', 'platform', 'machine', 'driver_sha256'):
        if old['metadata'][k] != new['metadata'][k]: raise ValueError('async host/helper differs')
    builds = [r['metadata']['build'] for r in (old, new)]
    for k in ('rustc', 'profile', 'environment'):
        if builds[0][k] != builds[1][k]: raise ValueError('async build configuration differs')
    before, after = (b['inputs'] for b in builds)
    fixed = [k for k in set(before) | set(after) if not k.startswith('crates/napi-vm-core/src/')]
    if any(before.get(k) != after.get(k) for k in fixed):
        raise ValueError('async harness/dependency/config inputs differ')
    right = {(r['engine'], r['mode'], r['workload']): r for r in new['results']}
    return dict(schema=1, changed_inputs=[k for k in sorted(set(before) | set(after)) if before.get(k) != after.get(k)], results=[dict(engine=x['engine'], mode=x['mode'], workload=x['workload'], before=x['median'], after=right[x['engine'], x['mode'], x['workload']]['median'], after_over_before=right[x['engine'], x['mode'], x['workload']]['median']/x['median']) for x in old['results']])


def ratios(path):
    r = json.loads(path.read_text()); workloads = validate(r)
    rows = {(x['engine'], x['mode'], x['workload']): x for x in r['results']}
    return dict(schema=1, source_sha256=sha(path), results=[dict(workload=n, mode=m, napi_vm=rows['napi-vm', m, n]['median'], rquickjs=rows['rquickjs', m, n]['median'], napi_vm_over_rquickjs=rows['napi-vm', m, n]['median']/rows['rquickjs', m, n]['median']) for n in workloads for m in MODES], job_counts=r['runs'][0]['job_counts'], limitations=r['metadata']['limitations'])


def main():
    p = argparse.ArgumentParser(description=__doc__); sub = p.add_subparsers(dest='command', required=True)
    b = sub.add_parser('build'); b.add_argument('--output', type=Path, required=True)
    r = sub.add_parser('run'); r.add_argument('--build', type=Path, required=True); r.add_argument('--output', type=Path, required=True)
    r.add_argument('--samples', type=int, default=15); r.add_argument('--iterations', type=int, default=10); r.add_argument('--processes', type=int, default=3)
    c = sub.add_parser('compare'); c.add_argument('before', type=Path); c.add_argument('after', type=Path)
    q = sub.add_parser('ratios'); q.add_argument('report', type=Path)
    a = p.parse_args()
    try:
        if a.command == 'build': build(a.output)
        elif a.command == 'run':
            if a.samples <= 0 or a.samples % 2 != 1 or a.iterations <= 0 or a.processes <= 0 or a.processes % 2 != 1: p.error('positive settings required; samples/processes must be odd')
            run(a.build, a.output, a.samples, a.iterations, a.processes)
        else: print(json.dumps(compare(a.before, a.after) if a.command == 'compare' else ratios(a.report), indent=2))
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        p.exit(1, str(error)+'\n')


if __name__ == '__main__': main()
