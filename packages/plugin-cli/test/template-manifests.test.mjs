import test from 'node:test';
import assert from 'node:assert/strict';
import { cp, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { main } from '../src/index.mjs';
import { capture } from '../../../tools/plugins/capture.mjs';

test('Rust scaffolds hide manifests from Cargo discovery and restore standalone path dependencies', async () => {
  const template = new URL('../templates/rust/', import.meta.url);
  async function manifests(directory) {
    const entries = await readdir(directory, {withFileTypes: true});
    return (await Promise.all(entries.map(async entry => entry.isDirectory()
      ? manifests(new URL(entry.name + '/', directory))
      : entry.name === 'Cargo.toml' ? [entry.name] : []))).flat();
  }
  assert.deepEqual(await manifests(template), []);
  const directory = await mkdtemp(join(tmpdir(), 'rust-template-manifests-'));
  try {
    const project = join(directory, 'project');
    await main(['create', project, '--language', 'rust']);
    for (const relative of ['Cargo.toml', 'vendor/napi-vm-plugin-sdk/Cargo.toml', 'vendor/napi-vm-plugin-protocol/Cargo.toml']) {
      assert.match(await readFile(join(project, relative), 'utf8'), /\[package\]/);
      await assert.rejects(readFile(join(project, relative + '.template')), {code: 'ENOENT'});
    }
    const result = await capture('cargo', ['metadata', '--no-deps', '--format-version', '1', '--manifest-path', join(project, 'Cargo.toml')], {cwd: directory});
    assert.match(JSON.stringify(result), /trusted-greeter-template/);
  } finally { await rm(directory, {recursive: true, force: true}); }
});
