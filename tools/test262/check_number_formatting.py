#!/usr/bin/env python3
"""Compare deterministic binary64 formatting cases with an independent Node oracle."""
import argparse
import json
import math
import random
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', required=True)
    parser.add_argument('--node', default='node')
    args = parser.parse_args()
    rng = random.Random(262)
    values = [0., -0., 5e-324, 1.7976931348623157e308, 1.005, 2.5, 9.99]
    for exponent in range(-320, 309, 17):
        value = 10. ** exponent
        values.extend([value, math.nextafter(value, 0.), math.nextafter(value, math.inf)])
    for _ in range(200):
        value = struct.unpack('>d', rng.getrandbits(64).to_bytes(8, 'big'))[0]
        if math.isfinite(value):
            values.append(value)
    expressions = []
    for value in values:
        for method, counts in [
            ('toPrecision', [1, 2, 17, 50, 100]),
            ('toExponential', [0, 1, 16, 50, 100]),
            ('toFixed', [0, 2, 20, 100]),
        ]:
            expressions.extend(f'({value!r}).{method}({count})' for count in counts)
        expressions.extend(f'({value!r}).{method}()'
                           for method in ['toPrecision', 'toExponential'])
    for offset in range(0, len(expressions), 30):
        chunk = expressions[offset:offset + 30]
        array = '[' + ','.join(chunk) + ']'
        expected = subprocess.check_output(
            [args.node, '-e', 'process.stdout.write(JSON.stringify(' + array + '))'],
            text=True,
        )
        source = (
            'var actual=' + array + ';var expected=' + expected + ';'
            'for(var i=0;i<actual.length;i++){'
            'if(actual[i]!==expected[i])throw Error("case ' + str(offset)
            + '+"+i+": "+actual[i]+" != "+expected[i]);}'
        )
        result = subprocess.run(
            [args.engine], input=json.dumps({'source': source}), capture_output=True,
            text=True, timeout=10, check=True,
        )
        report = json.loads(result.stdout)
        if report['status'] != 'ok':
            raise RuntimeError((chunk, report))
    print(json.dumps({
        'seed': 262, 'finite_values': len(values), 'comparisons': len(expressions),
        'mismatches': 0,
        'node_version': subprocess.check_output([args.node, '--version'], text=True).strip(),
    }))


if __name__ == '__main__':
    main()
