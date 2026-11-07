#!/usr/bin/env python3
"""Collect and compare a finite, local performance baseline. See docs/performance.md."""
import argparse, datetime, hashlib, json, math, os, pathlib, platform, re, shutil, statistics, subprocess, sys
from collection_report import read_report as read_collections
from public_api_report import read_report as read_public_api
import collection_memory

ROOT = pathlib.Path(__file__).resolve().parent.parent

def call(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, **kwargs)

def output(args):
    return call(args, stdout=subprocess.PIPE).stdout.strip()

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def summary(samples):
    if not samples or any(not isinstance(x, (int, float)) or isinstance(x, bool) or not math.isfinite(x) or x <= 0 for x in samples):
        raise ValueError('missing, non-finite, or non-positive timing sample')
    median = statistics.median(samples)
    return {'median': median, 'mad': statistics.median(abs(x-median) for x in samples), 'min': min(samples), 'max': max(samples), 'count': len(samples)}

def fingerprint(inputs_only=False):
    paths = output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard']).split('\0')
    prefixes = ('src/', 'crates/', 'packages/', 'examples/', 'bench/', 'benches/', 'tools/', 'Cargo.toml', 'Cargo.lock', 'package.json', 'package-lock.json', 'contracts/', 'build.rs', '.cargo/', 'index.js', 'index.mjs', 'index.d.ts', 'tsconfig')
    digest = hashlib.sha256()
    for name in sorted(set(filter(None, paths))):
        p = ROOT / name
        if p.is_file() and not name.startswith('bench/baselines/') and (not inputs_only or (name.startswith(prefixes) and not name.endswith('.md') and not name.startswith('tools/docs/'))):
            digest.update(name.encode()+b'\0'+bytes.fromhex(sha(p)))
    return digest.hexdigest()

def run(out):
    out = out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    raw = out / 'raw'
    raw.mkdir()
    git_status = output(['git', 'status', '--porcelain=v1', '--untracked-files=all'])
    source_hash = fingerprint()
    inputs_hash = fingerprint(True)
    # A partial local install may omit npm workspace links. Create only missing
    # links; never replace a caller's installed package.
    scope = ROOT/'node_modules/@napi-vm'
    scope.mkdir(parents=True, exist_ok=True)
    for name in ['plugin-protocol','plugin-sdk','plugin-host','plugin-codegen','plugin-cli']:
        link = scope/name
        target = ROOT/'packages'/name
        if not link.exists() and not link.is_symlink():
            link.symlink_to(target, target_is_directory=True)
        if link.resolve() != target.resolve():
            raise RuntimeError(f'workspace link points elsewhere: {link}')
    env = dict(os.environ)
    # Features and build profile are explicit. Cargo will rebuild any changed sources.
    builds = [(['cargo','build','--locked','--release','--lib','--bin','napi-vm-lsp'], 'cargo-build.log'), (['cargo','build','--locked','--release','--features','runtime','--example','scheduler_bench'], 'scheduler-build.log'), (['cargo','bench','--locked','--bench','vm','--no-run'], 'cargo-bench-build.log'), (['npm','run','plugins:build:ts'], 'plugin-build.log')]
    lib = ROOT/'target/release/libnapi_vm.so'
    addon = raw/'napi-vm.node'
    for args, name in builds:
        with (raw/name).open('w') as log:
            call(args, stdout=log, stderr=subprocess.STDOUT)
        if name == 'cargo-build.log':
            if not lib.exists():
                raise RuntimeError('this baseline currently requires a Linux release .so')
            shutil.copy2(lib, addon)
            built_lib_sha = sha(lib)
    env['NAPI_BENCH_BINDING'] = str(addon)
    env['NAPI_RS_NATIVE_LIBRARY_PATH'] = str(addon)
    plugin_outputs = [sorted((ROOT/'packages'/name/'dist').rglob('*.js')) for name in ['plugin-protocol','plugin-sdk','plugin-host']]
    plugin_hash = hashlib.sha256()
    for files in plugin_outputs:
        for path in files:
            plugin_hash.update(str(path.relative_to(ROOT)).encode()+b'\0'+bytes.fromhex(sha(path)))
    bench_match = re.search(r'Executable benches/vm\.rs \(([^)]+)\)', (raw/'cargo-bench-build.log').read_text())
    if not bench_match: raise RuntimeError('cannot identify built Criterion executable')
    criterion_binary = ROOT/bench_match.group(1)
    backend = {'criterion_sha256': sha(criterion_binary), 'trusted_emitted_js_sha256': plugin_hash.hexdigest(), 'addon_path': str(addon), 'addon_sha256': sha(addon), 'built_library_sha256_at_build': built_lib_sha, 'scheduler_sha256': sha(ROOT/'target/release/examples/scheduler_bench'), 'lsp_sha256': sha(ROOT/'target/release/napi-vm-lsp')}
    rows = []
    with (raw/'node.json').open('w') as log:
        call(['node','bench/performance/node.cjs'], env=env, stdout=log)
    node = json.loads((raw/'node.json').read_text())
    if node['bindingPath'] != str(addon): raise RuntimeError('wrong NAPI binding selected')
    rows.extend(node['results'])
    with (raw/'collections.json').open('w') as log:
        call(['node','bench/performance/collections.cjs'], env=env, stdout=log)
    collections, collection_rows = read_collections(raw/'collections.json')
    if collections['metadata']['bindingPath'] != str(addon) or collections['metadata']['addonSha256'] != backend['addon_sha256']:
        raise RuntimeError('wrong collection benchmark binding selected')
    for name, row in collection_rows.items():
        if not row['sharedIdentityPreserved']:
            raise RuntimeError('collection identity regression: '+name)
        rows.append({**row, 'name':'node/collections/'+name, 'backend':'NAPI retained call and outbound collection conversion'})
    with (raw/'public-api.json').open('w') as log:
        call(['node','bench/performance/public-api.cjs'], env=env, stdout=log)
    public_api, public_rows = read_public_api(raw/'public-api.json')
    if public_api['metadata']['bindingPath'] != str(addon) or public_api['metadata']['addonSha256'] != backend['addon_sha256']:
        raise RuntimeError('wrong public API benchmark binding selected')
    rows.extend(public_rows.values())
    collection_memory.run(addon, raw/'owner-memory.json', 3)
    owner_memory = json.loads((raw/'owner-memory.json').read_text())
    if owner_memory['metadata']['addon_sha256'] != backend['addon_sha256']:
        raise RuntimeError('wrong owner-memory benchmark binding selected')
    for row in owner_memory['results']:
        rows.append({'name':'node_owner_memory/'+row['name']+'/last_collected_rss',
                     'unit':'bytes', 'samples':[collection_memory.metrics(run)['last_collected_rss_bytes'] for run in row['runs']],
                     'backend':'Node process RSS after three collection-replacement rounds and explicit native GC; separate owner/process per sample'})
    # Unique Criterion baseline keeps previous runs intact and identifies this run's samples.
    label = 'perf_' + datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%d%H%M%S%f')
    criterion_cmd = ['cargo','bench','--locked','--bench','vm','--','--sample-size','10','--warm-up-time','0.2','--measurement-time','0.5','--save-baseline',label]
    with (raw/'criterion.log').open('w') as log:
        call(criterion_cmd, stdout=log, stderr=subprocess.STDOUT)
    criterion_root = ROOT/'target/criterion'
    samples = sorted(criterion_root.glob(f'**/{label}/sample.json'))
    if not samples: raise RuntimeError('Criterion produced no samples')
    for sample in samples:
        relative = sample.relative_to(criterion_root)
        dest = raw/'criterion'/relative
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(sample, dest)
        obj = json.loads(sample.read_text())
        values = [t/i for t,i in zip(obj['times'],obj['iters']) if i]
        name = '/'.join(relative.parts[:-2])
        rows.append({'name':'criterion/'+name,'unit':'ns/iteration','samples':values,'backend':'AST fresh' if name.startswith('run/') else 'bytecode asserted' if name.startswith('tiers/') and not name.endswith('gc_collect') else 'see suite source'})
    # Independent repetitions retain each whole-run summary, as these older tools do not emit their individual internal timings.
    for name, cmd, repeats in [('scheduler-rust',['target/release/examples/scheduler_bench'],3), ('scheduler-node-legacy',['node','bench/scheduler-async.cjs'],3), ('scheduler-node-persistent',['node','bench/scheduler-async.cjs','persistent'],3), ('trusted-node',['node','tools/plugins/benchmark.mjs'],3)]:
        collected = {}
        for i in range(repeats):
            log = raw/f'{name}-{i}.jsonl'
            with log.open('w') as f:
                call(cmd, env={**env, 'PLUGIN_BENCH_SAMPLES':'10'}, stdout=f)
            if name == 'trusted-node':
                obj=json.loads(log.read_text())
                if obj.get('selectedRuntime') is None: raise RuntimeError('trusted runtime missing')
                for p in obj['payloads']:
                    collected.setdefault('payload_%s_p50' % p['payloadBytes'], []).append(p['p50Ms']*1e6)
                collected.setdefault('startup', []).append(obj['startupThroughReadyMs']*1e6)
            else:
                for line in log.read_text().splitlines():
                    obj=json.loads(line)
                    collected.setdefault(obj['workload'], []).append(obj['batch_p50_us' if name == 'scheduler-rust' else 'p50_ms'] * (1000 if name == 'scheduler-rust' else 1e6))
        for workload, values in collected.items():
            rows.append({'name': name+'/'+workload, 'unit':'ns/batch' if name=='scheduler-rust' else 'ns/call' if name.startswith('scheduler-node') else 'ns', 'samples':values, 'backend':name+'; individual samples are per-run p50'})
    for row in rows: row['summary'] = summary(row['samples'])
    inputs_hash_end = fingerprint(True)
    source_hash_end = fingerprint()
    status_end = output(['git', 'status', '--porcelain=v1', '--untracked-files=all'])
    if inputs_hash_end != inputs_hash:
        raise RuntimeError('benchmark build inputs changed during run; discarded')
    meta = {'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(), 'git_head':output(['git','rev-parse','HEAD']), 'git_status_porcelain_start':git_status, 'git_status_porcelain_end':status_end, 'source_sha256_start':source_hash, 'source_sha256_end':source_hash_end, 'build_inputs_sha256_start':inputs_hash, 'build_inputs_sha256_end':inputs_hash_end, 'platform':platform.platform(), 'machine':platform.machine(), 'cpu':next((line.split(':',1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown'), 'cpu_count':os.cpu_count(), 'load_average_at_end':os.getloadavg(), 'build_environment':{k:v for k,v in sorted(os.environ.items()) if k in ('RUSTFLAGS','RUSTC_WRAPPER','CARGO_BUILD_TARGET','CARGO_TARGET_DIR','RUSTUP_TOOLCHAIN') or k.startswith('CARGO_PROFILE_')}, 'rustc':output(['rustc','--version','--verbose']), 'cargo':output(['cargo','--version']), 'node':output(['node','--version']), 'features':{'library':'default (napi)', 'criterion':'default (napi), plus core test-hooks dev feature', 'scheduler_rust':'default plus runtime', 'trusted_process':'emitted TypeScript on Node'}, 'profile':'release / Cargo bench profile', 'criterion':{'samples':10,'warmup_seconds':0.2,'measurement_seconds':0.5}, 'backend_artifacts':backend, 'limitations':['Finite representative workloads, not full code coverage.', 'No CPU affinity or isolation; compare on the same quiet machine.', 'Criterion samples are per-iteration batch averages. Scheduler and trusted samples are independent whole-run summary medians, not per-call samples.', 'Trusted process measurement uses the current emitted TypeScript sources, including any dirty edits.', 'LanguageService cases use the NAPI binding; LSP transport is built and fingerprinted but not measured.', 'No filesystem/network runtime, wasm, Bun, or native trusted-process workload measured.']}
    (out/'baseline.json').write_text(json.dumps({'schema':1,'metadata':meta,'results':rows}, indent=2)+'\n')
    print(out/'baseline.json')

def read_report(path):
    report = json.loads(path.read_text())
    if report.get('schema') != 1 or not isinstance(report.get('results'), list) or not report['results']:
        raise ValueError(f'{path}: invalid schema or empty results')
    rows = {}
    for row in report['results']:
        name = row.get('name')
        if not isinstance(name, str) or not name or name in rows:
            raise ValueError(f'{path}: missing or duplicate workload name')
        actual = summary(row.get('samples', []))
        stored = row.get('summary', {})
        if not isinstance(stored.get('median'), (float, int)) or not math.isclose(actual['median'], stored['median'], rel_tol=1e-12, abs_tol=0):
            raise ValueError(f'{path}: invalid median for {name}')
        rows[name] = row
    return report, rows

def compare(a,b):
    old,left = read_report(a)
    new,right = read_report(b)
    result=[]
    for name in sorted(left.keys() & right.keys()):
        x,y=left[name],right[name]
        if x['unit'] != y['unit'] or x['backend'] != y['backend']: raise ValueError('incompatible workload: '+name)
        result.append({'name':name, 'unit':x['unit'], 'baseline_median':x['summary']['median'], 'current_median':y['summary']['median'], 'ratio':y['summary']['median']/x['summary']['median']})
    differences = {k:{'baseline':old['metadata'].get(k), 'current':new['metadata'].get(k)} for k in ['platform','machine','cpu','cpu_count','rustc','cargo','node','features','profile','criterion','build_environment'] if old['metadata'].get(k) != new['metadata'].get(k)}
    print(json.dumps({'schema':1,'metadata_differences':differences,'warning':'Ratios are descriptive, not a significance test; compare metadata and noise.','removed':sorted(left.keys()-right.keys()),'added':sorted(right.keys()-left.keys()),'results':result},indent=2))

p=argparse.ArgumentParser(); sub=p.add_subparsers(dest='cmd',required=True)
r=sub.add_parser('run'); r.add_argument('--output',type=pathlib.Path,required=True)
c=sub.add_parser('compare'); c.add_argument('baseline',type=pathlib.Path); c.add_argument('current',type=pathlib.Path)
a=p.parse_args()
try:
    run(a.output) if a.cmd == 'run' else compare(a.baseline,a.current)
except (subprocess.CalledProcessError, OSError, ValueError, RuntimeError) as exc:
    print(f'performance baseline failed: {exc}',file=sys.stderr); sys.exit(1)
