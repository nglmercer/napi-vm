import { test, expect, afterEach } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync, symlinkSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  DEFAULT_MAX_FILE_BYTES,
  ResourceLimitError,
} from "../../plugins";
import { createNodeFileSystem } from "../../plugins/node";
import { cleanup, makeHost, makePlugin, manifestWith } from "./helpers";

afterEach(cleanup);

const scratch: string[] = [];
function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "napi-vm-limits-"));
  scratch.push(dir);
  return dir;
}
afterEach(() => {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

// ── read/write byte limits ───────────────────────────────────────────

test("readText refuses a file above the limit", () => {
  const dir = tempDir();
  const path = join(dir, "big.txt");
  writeFileSync(path, "x".repeat(2048));

  const fs = createNodeFileSystem({ maxReadBytes: 1024 });
  expect(() => fs.readText(path)).toThrow(ResourceLimitError);
  expect(() => fs.readText(path)).toThrow(/1024 byte read limit/);
});

test("readText allows a file exactly at the limit", () => {
  const dir = tempDir();
  const path = join(dir, "exact.txt");
  writeFileSync(path, "x".repeat(1024));

  const fs = createNodeFileSystem({ maxReadBytes: 1024 });
  expect(fs.readText(path)).toHaveLength(1024);
});

test("writeText refuses contents above the limit and writes nothing", () => {
  const dir = tempDir();
  const path = join(dir, "out.txt");

  const fs = createNodeFileSystem({ maxWriteBytes: 16 });
  expect(() => fs.writeText(path, "x".repeat(17))).toThrow(ResourceLimitError);
  // The limit is checked before the file is opened, so no truncated file is
  // left behind.
  expect(fs.exists(path)).toBe(false);
});

test("the write limit counts UTF-8 bytes, not characters", () => {
  const dir = tempDir();
  const path = join(dir, "utf8.txt");

  const fs = createNodeFileSystem({ maxWriteBytes: 8 });
  // 4 characters, 12 bytes.
  expect(() => fs.writeText(path, "你好你好")).toThrow(
    /8 byte write limit/,
  );
  // 8 characters, 8 bytes.
  expect(() => fs.writeText(path, "abcdefgh")).not.toThrow();
});

test("the default limit is 8 MiB", () => {
  expect(DEFAULT_MAX_FILE_BYTES).toBe(8 * 1024 * 1024);
});

test("a ResourceLimit error reaches the guest as a catchable error", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: ["*"] } }),
    entry: `
import { readText } from "napi:fs";
export default { onLoad() {} };
`,
    files: { "big.txt": "x".repeat(4096) },
  });

  const host = makeHost({ fs: createNodeFileSystem({ maxReadBytes: 1024 }) });
  const plugin = host.load(dir);
  expect(() =>
    plugin.vm.callFunction("__cap_fs_readText", ["./big.txt"]),
  ).toThrow(/ResourceLimit: file is larger than the 1024 byte read limit/);
});

// ── non-regular files ────────────────────────────────────────────────

test("readText refuses a directory rather than reporting a permission problem", () => {
  const dir = tempDir();
  const fs = createNodeFileSystem();
  expect(() => fs.readText(dir)).toThrow(/not a regular file|EISDIR/);
});

// ── symlink hardening ────────────────────────────────────────────────

test.if(process.platform !== "win32")(
  "readText refuses to follow a symlink at the final component",
  () => {
    const dir = tempDir();
    const secret = join(dir, "secret.txt");
    writeFileSync(secret, "classified");
    const link = join(dir, "link.txt");
    symlinkSync(secret, link);

    // The permission layer only ever hands down canonical paths, so a symlink
    // arriving here means it was swapped in after the check.
    const fs = createNodeFileSystem();
    expect(() => fs.readText(link)).toThrow(/ELOOP|not a regular file/);
  },
);

test.if(process.platform !== "win32")(
  "writeText refuses to follow a symlink at the final component",
  () => {
    const dir = tempDir();
    const outside = join(dir, "outside");
    mkdirSync(outside);
    const target = join(outside, "target.txt");
    writeFileSync(target, "original");
    const link = join(dir, "link.txt");
    symlinkSync(target, link);

    const fs = createNodeFileSystem();
    expect(() => fs.writeText(link, "overwritten")).toThrow(/ELOOP/);
  },
);
