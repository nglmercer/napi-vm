#!/usr/bin/env python3
"""Compare contextual binding syntax with Node; this does not execute guest code."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

NAMES = ('from', 'as', 'of', 'get', 'set', 'async', 'constructor',
         'undefined', 'await', 'yield', 'let', 'static', 'eval', 'arguments')
SCRIPT_FORMS = ('var {n};', 'let {n};', 'const {n}=1;', 'function {n}(){{}}',
                'var f=function {n}(){{}};', 'class {n}{{}}', 'var C=class {n}{{}};',
                'function f({n}){{}}', 'try{{}}catch({n}){{}}', '({n})=>1;',
                'var {n}=1;({{ {n} }});')
MODULE_FORMS = SCRIPT_FORMS + ('import {n} from "m";', 'import * as {n} from "m";',
                             'import {{value as {n}}} from "m";',
                             'export function {n}(){{}}', 'export class {n}{{}}')
NODE_DRIVER = r"""
const vm = require('node:vm');
if (typeof vm.SourceTextModule !== 'function') throw Error('Module parser unavailable');
let input = '';
process.stdin.on('data', data => input += data);
process.stdin.on('end', () => {
  for (const line of input.trim().split('\n')) {
    const request = JSON.parse(line);
    let accepted = true;
    try {
      if (request.module) new vm.SourceTextModule(request.source);
      else new vm.Script(request.source);
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error;
      accepted = false;
    }
    process.stdout.write(JSON.stringify({test: request.test, variant: request.variant, accepted}) + '\n');
  }
});
"""


def requests():
    rows = []
    for goal, forms in [('script', SCRIPT_FORMS), ('strict', SCRIPT_FORMS), ('module', MODULE_FORMS)]:
        for name in NAMES:
            for form in forms:
                source = form.format(n=name)
                if goal == 'strict':
                    source = '"use strict";' + source
                rows.append({'test': str(len(rows)), 'variant': goal,
                             'module': goal == 'module', 'source': source})
    return rows


def outcomes(output, rows):
    parsed = [json.loads(line) for line in output.splitlines()]
    expected = [(row['test'], row['variant']) for row in rows]
    observed = [(row['test'], row['variant']) for row in parsed]
    if observed != expected or any(type(row.get('accepted')) is not bool for row in parsed):
        raise ValueError('incomplete, reordered or invalid source-audit outcomes')
    return parsed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', type=Path, required=True)
    parser.add_argument('--source-commit', required=True)
    parser.add_argument('--node', default='node')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    rows = requests()
    source = '\n'.join(json.dumps(row) for row in rows) + '\n'
    current = outcomes(subprocess.run([str(args.engine.resolve())], input=source,
                       text=True, capture_output=True, check=True, timeout=30).stdout, rows)
    reference = outcomes(subprocess.run([args.node, '--experimental-vm-modules', '-e', NODE_DRIVER],
                         input=source, text=True, capture_output=True, check=True, timeout=30).stdout, rows)
    results = [{**row, 'accepted': actual['accepted'], 'expected_acceptance': expected['accepted'],
                'error': actual.get('error')} for row, actual, expected in zip(rows, current, reference)]
    report = {'domain': 'source syntax only', 'source_commit': args.source_commit,
              'engine_sha256': hashlib.sha256(args.engine.read_bytes()).hexdigest(),
              'reference': subprocess.check_output([args.node, '--version'], text=True).strip(),
              'total': len(results), 'mismatches': sum(row['accepted'] != row['expected_acceptance'] for row in results),
              'limitations': ['No harness or guest execution; this is not AST/bytecode execution differential.'],
              'results': results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: value for key, value in report.items() if key != 'results'}, indent=2))
    if report['mismatches']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
