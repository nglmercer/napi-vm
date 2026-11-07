#!/usr/bin/env node
// Check repository-owned Markdown links without fetching external URLs.
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const args = process.argv.slice(2);
if (args.some((arg) => arg !== '--json')) {
  console.error('usage: node tools/docs/check.mjs [--json]');
  process.exit(2);
}
const listed = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root, encoding: 'utf8' });
const files = [...new Set(listed.split('\0'))].filter((name) => name.endsWith('.md') &&
  !name.startsWith('vendor/') && !name.includes('/node_modules/')).sort();
const cache = new Map();
function document(name) {
  if (cache.has(name)) return cache.get(name);
  const original = fs.readFileSync(path.join(root, name), 'utf8');
  let fence;
  const text = original.split('\n').map((line) => {
    const marker = line.match(/^\s{0,3}(`{3,}|~{3,})/);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = undefined;
      return '';
    }
    return fence ? '' : line;
  }).join('\n');
  const anchors = new Set();
  const duplicates = new Map();
  for (const match of text.matchAll(/^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const title = match[1].replace(/<[^>]*>/g, '').replace(/\[([^\]]+)\]\([^)]*\)/g, '$1').replace(/[`*_~]/g, '');
    const slug = title.toLowerCase().replace(/[^\p{L}\p{N}_\-\s]/gu, '').replace(/\s/g, '-');
    const count = duplicates.get(slug) ?? 0;
    anchors.add(count ? `${slug}-${count}` : slug);
    duplicates.set(slug, count + 1);
  }
  for (const match of text.matchAll(/<(?:a|[a-z][a-z0-9]*)\b[^>]*\b(?:id|name)=["']([^"']+)["'][^>]*>/gi)) anchors.add(match[1]);
  const result = { text, anchors };
  cache.set(name, result);
  return result;
}
const errors = [];
let checked = 0;
const inventory = { guides: [], packages: [], templates: [], generated: [], evidence: [] };
for (const name of files) {
  if (!fs.existsSync(path.join(root, name))) continue; // A tracked deletion is handled by Git.
  const kind = name.startsWith('packages/plugin-cli/templates/') ? 'templates' :
    name.includes('/evidence/') || name.startsWith('docs/implementation-evidence/') ? 'evidence' :
    name.includes('/generated/') ? 'generated' :
    /^(?:packages|crates)\//.test(name) ? 'packages' : 'guides';
  inventory[kind].push(name);
  const { text } = document(name);
  const searchable = text.replace(/`+[^`\n]*`+/g, '');
  const targets = [...searchable.matchAll(/!?\[[^\]\n]*\]\(\s*(<[^>]+>|[^\s)]+)(?:\s+["'][^\n]*?["'])?\s*\)/g),
    ...searchable.matchAll(/^\s{0,3}\[[^\]]+\]:\s*(<[^>]+>|\S+)/gm)];
  for (const match of targets) {
    let target = match[1].replace(/^<|>$/g, '');
    if (/^[a-z][a-z0-9+.-]*:|^\/\//i.test(target)) continue;
    checked++;
    const line = searchable.slice(0, match.index).split('\n').length;
    try {
      const [rawFile, rawFragment] = target.split('#', 2);
      const relative = decodeURIComponent(rawFile.split('?', 1)[0]);
      const absolute = relative ? path.resolve(relative.startsWith('/') ? root : path.dirname(path.join(root, name)), relative.replace(/^\//, '')) : path.join(root, name);
      const resolved = path.relative(root, absolute).split(path.sep).join('/');
      if (resolved.startsWith('../') || path.isAbsolute(resolved)) throw new Error('target escapes repository');
      if (!fs.existsSync(absolute)) throw new Error(`missing target ${resolved}`);
      if (rawFragment && resolved.endsWith('.md')) {
        const fragment = decodeURIComponent(rawFragment);
        if (!document(resolved).anchors.has(fragment)) throw new Error(`missing heading ${resolved}#${fragment}`);
      }
    } catch (error) {
      errors.push({ file: name, line, target, error: error.message });
    }
  }
}
const report = { documents: Object.values(inventory).reduce((n, rows) => n + rows.length, 0), localLinksChecked: checked, inventory, errors,
  exclusions: ['Third-party vendor/ documentation', 'node_modules/ documentation'],
  scope: 'Local Markdown targets and heading anchors; external URLs and fenced examples are not fetched or executed.' };
if (args.includes('--json')) console.log(JSON.stringify(report, null, 2));
else {
  console.log(`${report.documents} documents, ${checked} local links, ${errors.length} errors`);
  for (const error of errors) console.error(`${error.file}:${error.line}: ${error.target}: ${error.error}`);
}
process.exitCode = errors.length ? 1 : 0;
