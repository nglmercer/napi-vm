#!/usr/bin/env python3
"""Compare every selected variant without hiding missing outcomes or regressions."""
import argparse
import json
from pathlib import Path


def indexed(report):
    rows = {}
    for row in report['results']:
        key = (row['test'], row['variant'])
        if key in rows:
            raise ValueError(f'duplicate outcome: {key}')
        rows[key] = row
    if len(rows) != report['total']:
        raise ValueError('outcome count does not match report total')
    return rows


def compare(baseline, current):
    for field in ('revision', 'worker_jobs', 'timeout_seconds'):
        if baseline[field] != current[field]:
            raise ValueError(f'incompatible {field}')
    before, after = indexed(baseline), indexed(current)
    if before.keys() != after.keys():
        raise ValueError('reports select different variants')
    gained, lost, changed = [], [], []
    for key in sorted(before):
        old, new = before[key], after[key]
        if old['status'] == new['status']:
            continue
        row = {'test': key[0], 'variant': key[1], 'before': old, 'after': new}
        changed.append(row)
        if new['status'] == 'pass':
            gained.append(row)
        elif old['status'] == 'pass':
            lost.append(row)
    return {'revision': current['revision'], 'total': current['total'],
            'baseline_engine_sha256': baseline['engine_sha256'],
            'current_engine_sha256': current['engine_sha256'],
            'baseline_counts': baseline['counts'], 'current_counts': current['counts'],
            'new_passes': len(gained), 'lost_passes': len(lost),
            'newly_passing': gained, 'formerly_passing': lost,
            'all_status_transitions': changed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline', type=Path)
    parser.add_argument('current', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    try:
        result = compare(json.loads(args.baseline.read_text()), json.loads(args.current.read_text()))
    except (KeyError, ValueError) as error:
        parser.error(str(error))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: result[key] for key in ('total', 'new_passes', 'lost_passes', 'current_counts')}, indent=2))


if __name__ == '__main__':
    main()
