#!/usr/bin/env python3
"""Time the actual LSP position module without rebuilding the VM.

Requires a release Cargo build's url rlib; its path and digest are recorded.
This isolates position conversion, not full LSP request latency.
"""
import argparse
import datetime
import hashlib
import json
import os
import pathlib
import platform
import statistics
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
DRIVER = r'''
#[allow(dead_code)]
#[path = SOURCE_PATH]
mod text;
EXTRA_MODULE
use std::hint::black_box;
use std::time::Instant;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let batches: usize = args[1].parse().unwrap();
    let iterations: usize = args[2].parse().unwrap();
    for (name, line, lines) in [
        ("ascii_200_lines", "const value = 123;\n", 200),
        ("ascii_20000_lines", "const value = 123;\n", 20000),
        ("unicode_crlf_4000_lines", "const icon = \"🔥é\";\r\n", 4000),
    ] {
        let source = line.repeat(lines);
        let column = 16;
        let expected = (lines - 1) * line.len()
            + if name.starts_with("unicode") { "const icon = \"🔥".len() } else { column };
        type Convert = fn(&str, usize, usize) -> usize;
        let implementations: Vec<(&str, Convert)> = IMPLEMENTATIONS;
        for (_, convert) in &implementations {
            assert_eq!(convert(&source, lines - 1, column), expected);
            for _ in 0..100 {
                black_box(convert(black_box(&source), black_box(lines - 1), black_box(column)));
            }
        }
        let mut values = vec![Vec::new(); implementations.len()];
        for sample in 0..batches {
            // Alternate A/B order every sample to limit time/order drift.
            for step in 0..implementations.len() {
                let index = (step + sample) % implementations.len();
                let convert = implementations[index].1;
                let start = Instant::now();
                for _ in 0..iterations {
                    black_box(convert(black_box(&source), black_box(lines - 1), black_box(column)));
                }
                values[index].push(start.elapsed().as_nanos() as f64 / iterations as f64);
            }
        }
        for ((engine, _), samples) in implementations.iter().zip(values) {
            println!("{{\"name\":\"{name}\",\"implementation\":\"{engine}\",\"bytes\":{},\"unit\":\"ns/conversion\",\"samples\":{:?}}}", source.len(), samples);
        }
    }
}
'''


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def call(args, **kwargs):
    return subprocess.run(args, check=True, text=True, cwd=ROOT, **kwargs)


def run(args):
    source = args.source.resolve()
    # The dependency only serves unrelated URI helpers in the same module.
    # Allow an explicit selection and always identify the exact linked artifact.
    candidates = list((ROOT / 'target/release/deps').glob('liburl-*.rlib'))
    url = args.url_rlib.resolve() if args.url_rlib else max(candidates, key=lambda p: p.stat().st_mtime)
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    snapshot = out / 'text.rs'
    snapshot.write_bytes(source.read_bytes())
    driver = out / 'driver.rs'
    extra = ''
    implementations = 'vec![("candidate", text::position_to_offset)]'
    baseline_metadata = {}
    if args.baseline:
        baseline = out / 'baseline-text.rs'
        baseline.write_bytes(args.baseline.resolve().read_bytes())
        extra = '#[allow(dead_code)]\n#[path = ' + json.dumps(str(baseline)) + ']\nmod baseline;'
        implementations = 'vec![("baseline", baseline::position_to_offset), ("candidate", text::position_to_offset)]'
        baseline_metadata = {'baseline_source_path': str(args.baseline.resolve()), 'baseline_source_sha256': sha(baseline)}
    driver.write_text(DRIVER.replace('SOURCE_PATH', json.dumps(str(snapshot))).replace('EXTRA_MODULE', extra).replace('IMPLEMENTATIONS', implementations))
    binary = out / 'positions'
    command = ['rustc', '--edition=2024', '-C', 'opt-level=3', str(driver),
               '--extern', 'url=' + str(url), '-L', 'dependency=' + str(url.parent), '-o', str(binary)]
    call(command)
    # Exercise the module's actual boundary tests as well as the timed results.
    test_binary = out / 'position-tests'
    call([*command[:-2], '--test', '-o', str(test_binary)])
    with (out / 'tests.log').open('w') as log:
        call([str(test_binary)], stdout=log)
    raw = call([str(binary), str(args.samples), str(args.iterations)], stdout=subprocess.PIPE).stdout
    (out / 'samples.jsonl').write_text(raw)
    rows = [json.loads(line) for line in raw.splitlines()]
    for row in rows:
        values = row['samples']
        median = statistics.median(values)
        row['summary'] = {'median': median, 'mad': statistics.median(abs(x - median) for x in values),
                          'min': min(values), 'max': max(values), 'count': len(values)}
    report = {'schema': 1, 'metadata': {
        **baseline_metadata, 'utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'git_head': call(['git', 'rev-parse', 'HEAD'], stdout=subprocess.PIPE).stdout.strip(),
        'git_status': call(['git', 'status', '--short'], stdout=subprocess.PIPE).stdout.strip(),
        'source_path': str(source), 'source_sha256': sha(snapshot), 'driver_sha256': sha(driver),
        'binary_sha256': sha(binary), 'url_rlib_path': str(url), 'url_rlib_sha256': sha(url),
        'rustc': call(['rustc', '--version', '--verbose'], stdout=subprocess.PIPE).stdout.strip(),
        'command': command, 'platform': platform.platform(), 'cpu_count': os.cpu_count(),
        'cpu': next((line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')), 'unknown'),
        'affinity': sorted(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else None,
        'load_average_at_end': os.getloadavg(), 'samples': args.samples, 'iterations_per_sample': args.iterations,
        'boundary': 'position_to_offset on a cursor in the last line; source construction, assertions and output excluded',
        'limitations': ['Isolated LSP position conversion, not full request latency.', 'No CPU isolation; compare repeated runs on the same machine.'],
    }, 'results': rows}
    (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(out / 'report.json')


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source', type=pathlib.Path, default=ROOT / 'src/lsp/text.rs')
parser.add_argument('--url-rlib', type=pathlib.Path)
parser.add_argument('--baseline', type=pathlib.Path, help='interleave with an earlier snapshot of the same module')
parser.add_argument('--output', type=pathlib.Path, required=True)
parser.add_argument('--samples', type=int, default=11)
parser.add_argument('--iterations', type=int, default=1000)
arguments = parser.parse_args()
if arguments.samples < 1 or arguments.iterations < 1:
    parser.error('samples and iterations must be positive')
run(arguments)
