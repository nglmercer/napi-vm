import json
import pathlib
import subprocess
import sys

root = pathlib.Path('artifacts/test262')
revision = '5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81'
neighbors = [
    'built-ins/RegExp/prototype/exec',
    'built-ins/RegExp/prototype/test',
    'built-ins/RegExp/prototype/flags',
    'annexB/built-ins/RegExp/prototype/compile',
    'built-ins/TypedArrayConstructors/internals',
]
def run(label, engine, paths):
    output = root / (label + '.json')
    args = [sys.executable, 'tools/test262/run.py', '.napi-vm/test262', '--engine', engine, '--revision', revision, '--jobs', '2', '--timeout', '5', '--output', str(output)]
    for path in paths:
        args.extend(['--path', path])
    with (root / (label + '.log')).open('w') as log:
        result = subprocess.run(args, stdout=log, stderr=subprocess.STDOUT)
    report = json.loads(output.read_text())
    print(label, report['total'], report['counts'], 'exit', result.returncode, flush=True)

run('timeouts-after', 'artifacts/test262/candidate-worker', json.loads((root / 'timeout-paths.json').read_text()))
run('neighbors-before', 'artifacts/test262/baseline-worker', neighbors)
run('neighbors-after', 'artifacts/test262/candidate-worker', neighbors)
for before, after, out in [('timeouts-baseline', 'timeouts-after', 'timeout-comparison'), ('neighbors-before', 'neighbors-after', 'neighbor-comparison')]:
    result = subprocess.run([sys.executable, 'tools/test262/compare.py', str(root / (before + '.json')), str(root / (after + '.json')), '--output', str(root / (out + '.json'))], capture_output=True, text=True)
    print(out, result.stdout, result.stderr, flush=True)
