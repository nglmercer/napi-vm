#!/usr/bin/env python3
"""Group every non-passing Test262 variant without filtering the denominator."""
import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path


def summarize(report):
    groups = defaultdict(Counter)
    phases = Counter()
    for row in report['results']:
        parts = row['test'].split('/')
        group = '/'.join(parts[:3] if parts[0] == 'language' else parts[:2])
        groups[group][row['status']] += 1
        if row['status'] != 'pass':
            phases[row.get('engine', {}).get('phase', row['status'])] += 1
    ordered = sorted(groups.items(), key=lambda item: (-sum(n for status, n in item[1].items() if status != 'pass'), item[0]))
    return {
        'domain': report['domain'], 'suite': report['suite'],
        'revision': report['revision'], 'engine_sha256': report['engine_sha256'],
        'selection': report['selection'], 'total': report['total'],
        'counts': report['counts'], 'pass_percentage': report['pass_percentage'],
        'non_passing_phases': dict(phases),
        'groups': [{'path': name, 'total': sum(counts.values()),
                    'non_passing': sum(n for status, n in counts.items() if status != 'pass'),
                    'counts': dict(counts)} for name, counts in ordered],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('report', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    summary = summarize(json.loads(args.report.read_text()))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps({key: value for key, value in summary.items() if key != 'groups'}, indent=2))
    for group in summary['groups'][:20]:
        print(f"{group['non_passing']:6} / {group['total']:6}  {group['path']}")


if __name__ == '__main__':
    main()
