import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, writeFile, chmod, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { resolveLaunch } from '../dist/index.js';

test('runtime discovery yields to other host work during slow version probes', { skip: process.platform === 'win32' }, async () => {
  const directory = await mkdtemp(join(tmpdir(), 'napi-probe-'));
  const command = join(directory, 'node-probe');
  await writeFile(command, `#!${process.execPath}\nsetTimeout(() => process.stdout.write(process.argv.includes('--version') ? 'v24.0.0' : JSON.stringify({node:'24.0.0',napi:'10'})), 150);\n`);
  await chmod(command, 0o700);
  await writeFile(join(directory, 'main.mjs'), '');
  let ticks = 0;
  const timer = setInterval(() => ticks++, 10);
  try {
    const result = await resolveLaunch({ directory, manifest: {
      protocol: {major:1,minMinor:0,maxMinor:0},
      launch: {kind:'javascript',entry:'main.mjs',runtimes:['node'],preferredRuntime:'node'},
    } }, { runtimePaths: {node:command} });
    assert.equal(result.command, command);
    assert.ok(ticks >= 5, `host event loop stalled: only ${ticks} timer ticks during two 150ms probes`);
  } finally { clearInterval(timer); await rm(directory, {recursive:true,force:true}); }
});

test('shutdown during preflight cannot create a new plugin instance', async () => {
  const { TrustedPluginHost } = await import('../dist/index.js');
  const host = new TrustedPluginHost();
  let release;
  host.preflight = () => new Promise(resolve => { release = () => resolve({ resolved: {manifest:{}} }); });
  host.start = async () => { assert.fail('must not start a child after shutdown'); };
  const loading = host.load('unused');
  await host.shutdown();
  release();
  await assert.rejects(loading, error => error.code === 'NOT_READY');
  assert.deepEqual(host.list(), []);
});
