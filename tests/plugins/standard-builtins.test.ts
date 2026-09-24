import { test, expect } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { pathToFileURL } from "node:url";

import { defaultPolicy, PluginHost } from "../../plugins";
import { nodePlatform } from "../../plugins/node";

function runReference(runtime: "node" | "bun", entry: string): unknown {
  const script = `const plugin = (await import(${JSON.stringify(pathToFileURL(entry).href)})).default; process.stdout.write(plugin.onLoad());`;
  const result = spawnSync(runtime, ["--input-type=module", "-e", script], {
    cwd: join(entry, ".."),
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`${runtime} fixture failed: ${result.stderr || result.error?.message}`);
  }
  return JSON.parse(result.stdout);
}

test("standard Node built-ins match Node and Bun using the same guest module", () => {
  const root = mkdtempSync(join(tmpdir(), "napi-vm-node-builtins-"));
  const entry = join(root, "plugin.mjs");
  const config = join(root, "config.txt");
  const copy = join(root, "copy.txt");
  const input = "portable runtime compatibility\n";
  mkdirSync(root, { recursive: true });
  writeFileSync(config, input);

  const source = `
import { readFileSync, writeFileSync } from "node:fs";
import { basename, join, normalize, sep } from "node:path";
import { createHash } from "node:crypto";
import { performance } from "node:perf_hooks";

export default {
  onLoad() {
    const text = readFileSync(${JSON.stringify(config)}, "utf8");
    const writeResult = writeFileSync(${JSON.stringify(copy)}, text.toUpperCase());
    return JSON.stringify({
      input: text,
      copy: readFileSync(${JSON.stringify(copy)}, "utf8"),
      writeReturnedUndefined: writeResult === undefined,
      path: [join("alpha", "beta"), normalize("alpha/../beta"), basename("/a/b.txt"), sep],
      hash: createHash("sha256").update(text).digest("hex"),
      clock: [typeof performance.now(), performance.now() >= 0],
    });
  },
};
`;
  writeFileSync(entry, source);

  try {
    const references = {
      node: runReference("node", entry),
      bun: runReference("bun", entry),
    };

    const manifest = {
      name: "standard-builtins",
      version: "1.0.0",
      apiVersion: 1,
      entry: "./plugin.mjs",
      permissions: {
        fs: { read: "*", write: "*" },
        path: true,
        crypto: true,
        timers: true,
      },
    };
    writeFileSync(join(root, "plugin.json"), JSON.stringify(manifest));
    const host = new PluginHost({
      platform: nodePlatform(),
      policy: {
        ...defaultPolicy(),
        capabilities: { crypto: true, timers: true },
      },
    });
    const vmResult = JSON.parse(host.load(root).loadResult as string);

    expect(references.bun).toEqual(references.node);
    expect(vmResult).toEqual(references.node);
    expect(readFileSync(copy, "utf8")).toBe(input.toUpperCase());
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
