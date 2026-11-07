// Build and retain both SDK versions before running. This measures the public
// in-process harness boundary: validation, enqueueing and all listener delivery.
// Requires Node.js 22.14+ (findPackageJSON) and build.json/source snapshots next
// to each emitted SDK. See https://nodejs.org/api/module.html#modulefindpackagejsonspecifier-base.
// Example (paths must resolve the same unchanged plugin-protocol dependency):
// node tools/plugins/benchmark-event-queue.mjs --before artifacts/before/index.js \
//   --after artifacts/after/index.js --output artifacts/event-queue.json
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, writeFile, realpath, readdir } from 'node:fs/promises';
import { cpus, loadavg, platform, arch, release } from 'node:os';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';
import { execFileSync } from 'node:child_process';
import { findPackageJSON } from 'node:module';
import { eventQueueContract as contract } from './event-queue-contract.mjs';

const { values } = parseArgs({ options: {
  before: { type: 'string' }, after: { type: 'string' }, output: { type: 'string' },
  samples: { type: 'string', default: '15' }, events: { type: 'string', default: '64' },
} });
assert(values.before && values.after && values.output, '--before, --after and a new --output path are required');
const sampleCount = Number(values.samples), eventCount = Number(values.events);
assert(Number.isSafeInteger(sampleCount) && sampleCount >= 3 && sampleCount <= 10000);
assert(Number.isSafeInteger(eventCount) && eventCount >= 1 && eventCount <= 256);
assert(eventCount * (65536 + 64) <= 16 * 1024 * 1024, 'batch must fit the SDK queue byte limit');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const fingerprint = async path => ({ path, sha256: hash(await readFile(path)) });
const root = fileURLToPath(new URL('../../', import.meta.url));
const beforePath = resolve(values.before), afterPath = resolve(values.after);
const protocolPackage = await realpath(findPackageJSON('@napi-vm/plugin-protocol', pathToFileURL(beforePath)));
assert.equal(await realpath(findPackageJSON('@napi-vm/plugin-protocol', pathToFileURL(afterPath))), protocolPackage, 'SDK versions must share the same protocol package');
assert.equal(await realpath(findPackageJSON('@napi-vm/plugin-protocol', import.meta.url)), protocolPackage);
const protocolManifest = JSON.parse(await readFile(protocolPackage, 'utf8'));
assert.equal(protocolManifest.exports['.'].import, './dist/index.js', 'review the protocol import entry before measuring');
const protocolDirectory = join(dirname(protocolPackage), 'dist');
const protocolFiles = (await readdir(protocolDirectory, { recursive: true })).sort().filter(file => file.endsWith('.js')).map(file => join(protocolDirectory, file));
const builds = {};
for (const [phase, path] of Object.entries({ before: beforePath, after: afterPath })) {
  const buildPath = join(dirname(path), 'build.json');
  const manifest = JSON.parse(await readFile(buildPath, 'utf8'));
  assert.equal((await fingerprint(path)).sha256, manifest.emitted.sha256, `${phase} emitted SDK differs from build record`);
  assert.equal((await fingerprint(join(dirname(path), 'index.ts'))).sha256, manifest.source.sha256, `${phase} source snapshot differs from build record`);
  builds[phase] = manifest;
}
const inputPaths = [beforePath, afterPath, protocolPackage, ...protocolFiles, join(root, 'package-lock.json'),
  ...[beforePath, afterPath].flatMap(path => ['index.ts', 'build.json', 'package.json'].map(file => join(dirname(path), file))),
  fileURLToPath(import.meta.url), fileURLToPath(new URL('./event-queue-contract.mjs', import.meta.url))];
const fingerprints = await Promise.all(inputPaths.map(fingerprint));
const loadStart = loadavg();
const modules = { before: await import(pathToFileURL(beforePath).href), after: await import(pathToFileURL(afterPath).href) };
const turn = () => new Promise(resolve => setImmediate(resolve));
const median = values => { const sorted = [...values].sort((a, b) => a - b); const middle = sorted.length >> 1; return sorted.length % 2 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2; };
const summarize = values => { const mid = median(values); return { count: values.length, median: mid, mad: median(values.map(value => Math.abs(value - mid))), min: Math.min(...values), max: Math.max(...values) }; };

async function makeRunner(sdk, payloadBytes, subscriberCount) {
  let batch;
  const errors = [];
  const definition = sdk.definePlugin(contract, {}, { initialize(_, ctx) {
    for (let sub = 0; sub < subscriberCount; sub++) ctx.subscribe(contract, 'changed', value => {
      const counters = batch.counters[sub];
      counters.count++; counters.indexSum += value.index; counters.textBytes += value.text.length;
      counters.textEnds += value.text.charCodeAt(0) + value.text.charCodeAt(value.text.length - 1);
      if (++batch.received === eventCount * subscriberCount) batch.resolve();
    }, error => { errors.push(error); batch?.reject(error); });
  } });
  const harness = await sdk.createHarness(definition, { services: [sdk.definePlugin(contract, {})] });
  const payloads = Array.from({ length: eventCount }, (_, index) => ({ index, text: 'x'.repeat(payloadBytes) }));
  return {
    async run() {
      await turn(); // Let the preceding Tasks finalizers finish outside timing.
      let resolveBatch, rejectBatch;
      const completed = new Promise((resolve, reject) => { resolveBatch = resolve; rejectBatch = reject; });
      batch = { received: 0, resolve: resolveBatch, reject: rejectBatch,
        counters: Array.from({ length: subscriberCount }, () => ({ count: 0, indexSum: 0, textBytes: 0, textEnds: 0 })) };
      const deadline = setTimeout(() => rejectBatch(new Error('event batch did not drain')), 10000);
      const start = process.hrtime.bigint();
      try {
        for (const payload of payloads) await harness.emitHostEvent(contract, 'changed', payload);
        await completed;
        const elapsed = Number(process.hrtime.bigint() - start);
        assert.deepEqual(errors, []);
        for (const counters of batch.counters) assert.deepEqual(counters, {
          count: eventCount, indexSum: eventCount * (eventCount - 1) / 2,
          textBytes: payloadBytes * eventCount, textEnds: 240 * eventCount,
        });
        return elapsed / eventCount;
      } finally { clearTimeout(deadline); }
    },
    async close() { await harness.shutdown(); },
  };
}

const rows = [];
for (const payloadBytes of [64, 4096, 65536]) for (const subscribers of [1, 8]) {
  const runners = { before: await makeRunner(modules.before, payloadBytes, subscribers), after: await makeRunner(modules.after, payloadBytes, subscribers) };
  const samples = { before: [], after: [] }, orders = [];
  try {
    for (let warmup = 0; warmup < 5; warmup++) for (const phase of ['before', 'after']) await runners[phase].run();
    for (let sample = 0; sample < sampleCount; sample++) {
      const order = sample % 2 ? ['after', 'before'] : ['before', 'after']; orders.push(order);
      for (const phase of order) samples[phase].push(await runners[phase].run());
    }
  } finally { await runners.before.close(); await runners.after.close(); }
  const summaries = { before: summarize(samples.before), after: summarize(samples.after) };
  rows.push({ name: `harness_event_fanout/${payloadBytes}_bytes/${subscribers}_subscribers`,
    payloadBytes, serializedPayloadBytes: {
      min: Buffer.byteLength(JSON.stringify({ text: 'x'.repeat(payloadBytes), index: 0 })),
      max: Buffer.byteLength(JSON.stringify({ text: 'x'.repeat(payloadBytes), index: eventCount - 1 })),
    },
    subscribers, eventsPerSample: eventCount, unit: 'ns/event delivered to all subscribers',
    samples, summaries, orders, afterOverBefore: summaries.after.median / summaries.before.median });
}
for (const old of fingerprints) assert.equal((await fingerprint(old.path)).sha256, old.sha256, `input changed during benchmark: ${old.path}`);
const report = { schema: 1, metadata: {
  utc: new Date().toISOString(), gitHead: execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim(),
  gitStatus: execFileSync('git', ['status', '--short'], { cwd: root, encoding: 'utf8' }),
  node: process.version, versions: process.versions, platform: platform(), arch: arch(), osRelease: release(), cpu: cpus()[0]?.model,
  logicalCpus: cpus().length, loadAverageBefore: loadStart, loadAverageAfter: loadavg(),
  arguments: process.argv.slice(2), fingerprints, builds, contract, contractJsonSha256: hash(JSON.stringify(contract)),
  sampleCount, eventCount, warmupBatchesPerPhase: 5, boundary: 'createHarness.emitHostEvent validation + enqueue + all subscriber callbacks; reused harnesses; setup/shutdown/assertions excluded',
  limitations: ['Same Node process and unchanged protocol module; alternating phase order.',
    'Batch averages include Promise scheduling, validation and listener counters; no transport or child process is timed.',
    'ASCII strings; payloadBytes is text length and serialized JSON payload is slightly larger.',
    'No CPU affinity, forced GC or machine isolation; medians/MAD do not establish statistical significance.'],
}, rows };
await writeFile(resolve(values.output), JSON.stringify(report, null, 2) + '\n', { flag: 'wx' });
console.log(JSON.stringify({ output: resolve(values.output), rows: rows.map(({ name, afterOverBefore, summaries }) => ({ name, afterOverBefore, beforeMedian: summaries.before.median, afterMedian: summaries.after.median })) }, null, 2));
