'use strict';
// Build first, then select an addon explicitly; no generated-loader fallback.
const assert = require('node:assert/strict');
const { createHash } = require('node:crypto');
const { readFileSync } = require('node:fs');
const os = require('node:os');
const path = require('node:path');
if (!process.env.NAPI_BENCH_BINDING) throw new Error('Set NAPI_BENCH_BINDING to a freshly built addon');
const bindingPath = path.resolve(process.env.NAPI_BENCH_BINDING);
const { Vm } = require(bindingPath);
assert.equal(require.cache[bindingPath]?.filename, bindingPath);
const samples = Number(process.env.NAPI_BENCH_SAMPLES ?? 21);
const iterations = Number(process.env.NAPI_BENCH_ITERATIONS ?? 50);
assert(Number.isSafeInteger(samples) && samples > 0);
assert(Number.isSafeInteger(iterations) && iterations > 0);
const median = values => {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
};
const results = [];
for (const backingBytes of [64, 8 * 1024 * 1024, 32 * 1024 * 1024]) {
  const vm = new Vm();
  try {
    const source = `
      const backing = new ArrayBuffer(${backingBytes});
      const bytes = new Uint8Array(backing, backing.byteLength - 16, 16);
      for (let i = 0; i < bytes.length; i++) bytes[i] = i + 1;
      function getView() { return bytes; }
    `;
    vm.run(source);
    const expected = Array.from({ length: 16 }, (_, i) => i + 1);
    const check = value => {
      assert(value instanceof Uint8Array);
      assert.equal(value.byteOffset, 0);
      assert.equal(value.buffer.byteLength, 16);
      assert.deepEqual([...value], expected);
    };
    for (let i = 0; i < 20; i++) check(vm.callFunction('getView', []));
    const values = [];
    for (let i = 0; i < samples; i++) {
      let elapsed = 0n;
      for (let j = 0; j < iterations; j++) {
        const start = process.hrtime.bigint();
        const value = vm.callFunction('getView', []);
        elapsed += process.hrtime.bigint() - start;
        check(value);
      }
      values.push(Number(elapsed) / iterations);
    }
    const mid = median(values);
    results.push({ name: 'node_binding/typed_array_subview_' + backingBytes,
      source, backingBytes, visibleBytes: 16, unit: 'ns/call',
      backend: 'warm Vm.callFunction returning a fixed guest Uint8Array; correctness assertions excluded',
      samples: values, summary: { median: mid, mad: median(values.map(value => Math.abs(value - mid))),
        min: Math.min(...values), max: Math.max(...values), count: values.length } });
  } finally { vm.dispose(); }
}
console.log(JSON.stringify({ schema: 1, metadata: { utc: new Date().toISOString(), bindingPath,
  addonSha256: createHash('sha256').update(readFileSync(bindingPath)).digest('hex'),
  scriptSha256: createHash('sha256').update(readFileSync(__filename)).digest('hex'),
  node: process.version, platform: process.platform, arch: process.arch,
  cpu: os.cpus()[0]?.model, samples, iterationsPerSample: iterations,
  limitations: ['Per-sample averages of timed calls, excluding assertions and VM setup.', 'No CPU affinity or isolation.'] }, results }, null, 2));
