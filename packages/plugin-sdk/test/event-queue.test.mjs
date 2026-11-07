import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { TrustedPluginHost } from '../../plugin-host/dist/index.js';
import { eventQueueContract as E } from '../../../tools/plugins/event-queue-contract.mjs';

const G = JSON.parse(await readFile(new URL('../../../contracts/trusted-plugins/generated/greeter.contract.json', import.meta.url), 'utf8'));
// An explicit immutable module path also lets the benchmark review reproduce
// these failures against the pre-change emitted SDK.
const sdkModule = process.env.NAPI_VM_TEST_SDK ?? new URL('../dist/index.js', import.meta.url).href;
const { definePlugin, createHarness } = await import(sdkModule);
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const turn = () => new Promise(resolve => setImmediate(resolve));
const payload = (index, bytes = 64) => ({ text: 'x'.repeat(bytes), index });
const harnessWith = initialize => createHarness(definePlugin(G, { greet: () => ({ message: 'ok' }) }, { initialize }), { services: [definePlugin(E, {})] });

test('harness dequeues shared payloads even if another listener creates a cycle', { timeout: 10000 }, async () => {
  const delivered = deferred(), errors = [];
  const h = await harnessWith((_, ctx) => {
    ctx.subscribe(E, 'changed', value => { value.self = value; });
    ctx.subscribe(E, 'changed', value => { assert.equal(value.self, value); delivered.resolve(); }, error => { errors.push(error); delivered.resolve(); });
  });
  try {
    await h.emitHostEvent(E, 'changed', payload(0));
    await delivered.promise;
    assert.deepEqual(errors, []);
  } finally { await h.shutdown(); }
});

test('harness releases original queued bytes after another listener shrinks the payload', { timeout: 10000 }, async () => {
  const errors = []; let delivered;
  const h = await harnessWith((_, ctx) => {
    ctx.subscribe(E, 'changed', value => { value.text = ''; });
    ctx.subscribe(E, 'changed', () => delivered.resolve(), error => { errors.push(error); delivered.resolve(); });
  });
  try {
    for (let index = 0; index < 18; index++) {
      delivered = deferred();
      await h.emitHostEvent(E, 'changed', payload(index, 1024 * 1024));
      await delivered.promise;
      assert.deepEqual(errors, [], `idle queue retained bytes after event ${index}`);
      await turn();
    }
  } finally { await h.shutdown(); }
});

test('harness keeps bounded backpressure and stops only the overflowing subscriber', { timeout: 10000 }, async () => {
  const entered = deferred(), release = deferred(), overloaded = deferred();
  let calls = 0, healthy = 0;
  const h = await harnessWith((_, ctx) => {
    ctx.subscribe(E, 'changed', async () => { calls++; entered.resolve(); await release.promise; }, error => overloaded.resolve(error));
    ctx.subscribe(E, 'changed', () => { healthy++; });
  });
  try {
    await h.emitHostEvent(E, 'changed', payload(0)); await entered.promise;
    // The in-progress callback is excluded; exactly 256 waiting events fit.
    for (let index = 1; index <= 256; index++) { await h.emitHostEvent(E, 'changed', payload(index)); await turn(); }
    assert.equal(calls, 1);
    await h.emitHostEvent(E, 'changed', payload(257));
    assert.equal((await overloaded.promise).code, 'OVERLOADED');
    release.resolve(); await turn(); await turn();
    assert.equal(calls, 1, 'overflow must discard the stopped subscription queue');
    assert.equal(healthy, 258);
  } finally { release.resolve(); await h.shutdown(); }
});

test('harness enforces queued-byte limits and isolates listener/onError failures', { timeout: 10000 }, async () => {
  let calls = 0; const errors = [];
  const h = await harnessWith((_, ctx) => {
    ctx.subscribe(E, 'changed', () => { calls++; }, error => errors.push(error.code));
    ctx.subscribe(E, 'changed', () => { throw new Error('listener failed'); }, error => { errors.push(error.code); throw new Error('onError failed'); });
  });
  try {
    await h.emitHostEvent(E, 'changed', payload(0)); await turn(); await turn();
    assert.equal(calls, 1); assert.deepEqual(errors, ['INTERNAL_ERROR']);
    await h.emitHostEvent(E, 'changed', payload(1, 16 * 1024 * 1024));
    assert.equal(calls, 1); assert.deepEqual(errors, ['INTERNAL_ERROR', 'OVERLOADED']);
    await h.emitHostEvent(E, 'changed', payload(2)); await turn();
    assert.equal(calls, 1, 'stopped subscriptions must not be restarted');
  } finally { await h.shutdown(); }
});

async function processFixture(mode, limits, run) {
  const directory = await mkdtemp(join(tmpdir(), 'napi-sdk-event-queue-'));
  const host = new TrustedPluginHost({
    runtimePaths: { node: process.env.NAPI_VM_NODE ?? (process.versions.bun ? 'node' : process.execPath) }, limits,
  });
  const sdk = sdkModule;
  await writeFile(join(directory, 'main.mjs'), `
    import { serve, definePlugin } from ${JSON.stringify(sdk)};
    const G=${JSON.stringify(G)}, E=${JSON.stringify(E)}, mode=${JSON.stringify(mode)};
    const state={received:0,errors:[],entered:false}; let release;
    const gate=new Promise(resolve=>{release=resolve;});
    await serve(definePlugin(G,{greet:({name})=>{if(name==='release')release();return{message:JSON.stringify(state)};}},{
      initialize(_,ctx){
        if(mode==='cycle'||mode==='shrink')ctx.subscribe(E,'changed',value=>{if(mode==='cycle')value.self=value;else value.text='';});
        ctx.subscribe(E,'changed',async value=>{
          if(mode==='cycle'&&value.self!==value)throw new Error('payload identity changed');
          state.received++; state.entered=true; if(mode==='blocked')await gate;
        },error=>state.errors.push(error.code));
      },shutdown(){release();}
    }),{metadata:{id:'example.greeter',version:'0.1.0',requiresHost:[E]}});
  `);
  await writeFile(join(directory, 'greeter.json'), JSON.stringify(G));
  await writeFile(join(directory, 'plugin.json'), JSON.stringify({
    manifestVersion: 2, execution: 'trusted-process', id: 'example.greeter', version: '0.1.0',
    protocol: { major: 1, minMinor: 0, maxMinor: 0 }, provides: { 'example.greeter': '1.0.0' },
    requiresHost: { [E.descriptor.id]: E.descriptor.version }, contracts: ['greeter.json'],
    launch: { kind: 'javascript', entry: 'main.mjs', runtimes: ['node'], preferredRuntime: 'node', args: [] }, assets: [],
  }));
  host.registerService(E, {});
  let handle;
  try {
    handle = await host.load(directory, { runtime: 'node', development: true });
    const status = async (name = 'status') => JSON.parse((await handle.invoke(G, 'greet', { name })).message);
    const until = async condition => {
      for (let tries = 0; tries < 100; tries++) {
        const state = await status(); if (condition(state)) return state;
        await new Promise(resolve => setTimeout(resolve, 5));
      }
      assert.fail('event callback did not reach the expected state');
    };
    await run(handle, status, until);
  } finally {
    // Release callbacks before SDK shutdown waits for its tracked tasks.
    if (handle?.status === 'READY') await handle.invoke(G, 'greet', { name: 'release' }).catch(() => {});
    await host.shutdown(); await rm(directory, { recursive: true, force: true });
  }
}

test('production SDK dequeues a cyclic payload shared by another subscriber', { timeout: 15000 }, async () => {
  await processFixture('cycle', {}, async (handle, _status, until) => {
    await handle.emit(E, 'changed', payload(0));
    const state = await until(state => state.received || state.errors.length);
    assert.deepEqual(state.errors, []); assert.equal(state.received, 1);
  });
});

test('production SDK releases the enqueued size after shared payload mutation', { timeout: 15000 }, async () => {
  await processFixture('shrink', { maxQueuedBytes: 8192 }, async (handle, _status, until) => {
    for (let index = 0; index < 12; index++) {
      await handle.emit(E, 'changed', payload(index, 1024));
      const state = await until(state => state.received === index + 1 || state.errors.length);
      assert.deepEqual(state.errors, []); assert.equal(state.received, index + 1);
    }
  });
});

for (const [boundary, limits, size, waiting] of [
  ['count', { maxEventQueue: 2 }, 64, 2],
  ['bytes', { maxQueuedBytes: 4096 }, 1024, 3],
]) {
  test(`production SDK preserves ${boundary} backpressure while a listener is blocked`, { timeout: 15000 }, async () => {
    await processFixture('blocked', limits, async (handle, status, until) => {
      await handle.emit(E, 'changed', payload(0, size)); await until(state => state.entered);
      for (let index = 1; index <= waiting; index++) {
        await handle.emit(E, 'changed', payload(index, size));
        await handle.peer.request('system.ping', {}); // Flush transport without unblocking the subscriber.
      }
      assert.deepEqual((await status()).errors, []);
      await handle.emit(E, 'changed', payload(waiting + 1, size));
      const state = await until(state => state.errors.length);
      assert.deepEqual(state.errors, ['OVERLOADED']); assert.equal(state.received, 1);
      await status('release');
    });
  });
}
