import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import time

workers = {'before': Path('artifacts/test262/baseline-worker'), 'after': Path('target/ci-optimized/napi-vm-test262')}
results = {'workers': {name: hashlib.sha256(path.read_bytes()).hexdigest() for name,path in workers.items()}, 'cases': []}
for size in [5000, 10000, 20000]:
    source = f"function build(n){{var o={{length:n}};for(var k=0;k<o.length;k++)o[k]=k;return o[n-1];}}if(build({size})!=={size-1})throw new Error('wrong value');"
    samples = {name: [] for name in workers}
    for _ in range(3):
        for name,path in workers.items():
            start = time.perf_counter()
            result = subprocess.run([str(path)], input=json.dumps({'source':source}), text=True, capture_output=True, timeout=60, check=True)
            elapsed = time.perf_counter() - start
            report = json.loads(result.stdout)
            assert report['status'] == 'ok', report
            samples[name].append(elapsed)
    case = {'properties':size, 'source':source, 'samples_seconds':samples, 'median_seconds':{name:statistics.median(times) for name,times in samples.items()}}
    results['cases'].append(case)
    print(json.dumps(case), flush=True)
Path('artifacts/test262/dictionary-writes/benchmarks.json').write_text(json.dumps(results, indent=2)+'\n')
