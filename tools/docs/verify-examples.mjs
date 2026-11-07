#!/usr/bin/env node
// Execute the exact first-party Markdown blocks marked `javascript verify`
// and, with --rust, compile/run the `rust verify` blocks against this checkout.
import { readFileSync, writeFileSync, unlinkSync, existsSync } from 'node:fs';
import { cp, mkdir, mkdtemp, readFile, writeFile, rm } from 'node:fs/promises';
import { execFileSync, spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const args = process.argv.slice(2);
if (args.some(arg => arg !== '--rust')) {
  console.error('usage: node tools/docs/verify-examples.mjs [--rust]');
  process.exit(2);
}
const files = [...new Set(execFileSync('git',
  ['ls-files', '-z', '--cached', '--others', '--exclude-standard'],
  { cwd: root, encoding: 'utf8' }).split('\0'))]
  .filter(name => name.endsWith('.md') && !name.startsWith('vendor/') &&
    !name.includes('/node_modules/') && existsSync(join(root, name))).sort();
const examples = [];
for (const file of files) {
  const lines = readFileSync(join(root, file), 'utf8').split('\n');
  let fence;
  for (let index = 0; index < lines.length; index++) {
    const marker = lines[index].match(/^\s{0,3}(`{3,}|~{3,})(.*)$/);
    if (!fence && marker) {
      const info = marker[2].trim().split(/\s+/);
      fence = { marker: marker[1], language: info[0], verify: info[1] === 'verify',
        file, line: index + 2, source: [] };
    } else if (fence && marker && marker[1][0] === fence.marker[0] &&
               marker[1].length >= fence.marker.length && !marker[2].trim()) {
      if (fence.verify) {
        if (!['javascript', 'rust'].includes(fence.language)) {
          throw new Error(`${file}:${fence.line}: unsupported verified language`);
        }
        examples.push({ ...fence, source: fence.source.join('\n') });
      }
      fence = undefined;
    } else if (fence) {
      fence.source.push(lines[index]);
    }
  }
  if (fence) throw new Error(`${file}:${fence.line}: unclosed code fence`);
}
const javascript = examples.filter(example => example.language === 'javascript');
const rust = examples.filter(example => example.language === 'rust');
if (!javascript.length || !rust.length) throw new Error('Expected verified Node and Rust documentation examples');
// Build a JS-only, self-contained fixture outside the repository so the host
// example cannot accidentally resolve runtime dependencies from our workspace.
const temporary = await mkdtemp(join(tmpdir(), 'napi-vm-doc-examples-'));
try {
  const staging = join(temporary, 'source');
  await mkdir(staging);
  const fixture = join(root, 'examples/trusted/greeter-ts');
  for (const directory of ['dist', 'contracts']) {
    await cp(join(fixture, directory), join(staging, directory), { recursive: true });
  }
  await cp(join(fixture, 'package.json'), join(staging, 'package.json'));
  for (const name of ['plugin-sdk', 'plugin-protocol']) {
    const destination = join(staging, 'node_modules/@napi-vm', name);
    await mkdir(destination, { recursive: true });
    await cp(join(root, 'packages', name, 'dist'), join(destination, 'dist'), { recursive: true });
    for (const file of ['package.json', 'LICENSE']) await cp(join(root, 'packages', name, file), join(destination, file));
  }
  const manifest = JSON.parse(await readFile(join(fixture, 'plugin.json'), 'utf8'));
  delete manifest.development;
  manifest.extensions = { 'napi-vm.pack': { include: ['package.json', 'dist',
    'node_modules/@napi-vm/plugin-sdk', 'node_modules/@napi-vm/plugin-protocol'] } };
  await writeFile(join(staging, 'plugin.json'), JSON.stringify(manifest));
  const { pack } = await import('../../packages/plugin-cli/src/pack.mjs');
  const packaged = join(temporary, 'package');
  await pack(staging, packaged);
  for (const example of javascript) {
    const code = `(async () => {\n${example.source}\n})().catch(error => { console.error(error); process.exitCode = 1; });`;
    const result = spawnSync(process.execPath, ['--input-type=commonjs', '--eval', code],
      { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024,
        env: { ...process.env, PLUGIN_DIRECTORY: packaged } });
    if (result.error || result.status !== 0) {
      throw new Error(`${example.file}:${example.line}: Node example failed\n${result.error?.message ?? ''}\n${result.stdout ?? ''}${result.stderr ?? ''}`);
    }
    console.log(`PASS ${example.file}:${example.line}`);
  }
  if (args.includes('--rust')) {
    // Cargo must compile these examples with the facade's path dependencies and
    // lockfile. A unique temporary integration target avoids maintaining a second
    // copy of each example or a separate dependency graph.
    const name = `documentation_examples_${process.pid}`;
    const target = join(root, 'tests', `${name}.rs`);
    const source = rust.map((example, index) =>
      `// ${example.file}:${example.line}\nmod example_${index} {\n${example.source}\n#[test]\nfn verify() { main().expect("documentation example succeeds"); }\n}\n`).join('\n');
    writeFileSync(target, source, { flag: 'wx' });
    try {
      const result = spawnSync('cargo', ['test', '--locked', '--profile', 'ci-optimized',
        '--no-default-features', '--features', 'runtime', '--jobs', '1', '--test', name],
      { cwd: root, stdio: 'inherit' });
      if (result.error || result.status !== 0) throw result.error ?? new Error('Rust documentation examples failed');
    } finally {
      unlinkSync(target);
    }
  }
  console.log(`${javascript.length} Node examples passed; ${rust.length} Rust examples ${args.includes('--rust') ? 'passed' : 'available (use --rust)'}.`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
