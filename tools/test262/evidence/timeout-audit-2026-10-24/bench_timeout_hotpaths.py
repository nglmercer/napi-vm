import json
import pathlib
import statistics
import subprocess
import time

cases = {
    'regexp-literal': "for(var k=0;k<20000;k++){if(!/a/g.test('a'))throw new Error('wrong match');}",
    'typed-array-numeric-keys': "var a=new Uint8Array(10000);for(var n=0;n<10;n++){for(var k=0;k<10000;k++){a[k]=k;var x=a[k]}}if(a[9999]!==15)throw new Error('wrong value');",
}
workers = {
    'before': 'artifacts/test262/baseline-worker',
    'after': 'artifacts/test262/candidate-worker',
}
results = []
for name, source in cases.items():
    samples = {label: [] for label in workers}
    for _ in range(3):
        for label, worker in workers.items():
            start = time.perf_counter()
            result = subprocess.run([worker], input=json.dumps({'source': source}), text=True, capture_output=True, timeout=60, check=True)
            elapsed = time.perf_counter() - start
            report = json.loads(result.stdout)
            assert report['status'] == 'ok', report
            samples[label].append(elapsed)
    entry = {'name': name, 'source': source, 'samples_seconds': samples, 'median_seconds': {k: statistics.median(v) for k, v in samples.items()}}
    results.append(entry)
    print(json.dumps(entry), flush=True)
pathlib.Path('artifacts/test262/hotpath-benchmarks.json').write_text(json.dumps(results, indent=2) + '\n')
