import { test, expect, afterEach } from "bun:test";
import { mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cleanup, makeHost, makePlugin, manifestWith, outsideDir } from "./helpers";

afterEach(cleanup);

const ENTRY = "export default { onLoad() {} };";

/**
 * Build a plugin whose `cache/outside` is a symlink to a directory outside the
 * plugin root, holding a `secret.txt`.
 */
function escapingPlugin(permissions: unknown) {
  const dir = makePlugin({
    manifest: manifestWith(permissions),
    entry: ENTRY,
    files: { "cache/keep.txt": "keep" },
  });
  const target = join(outsideDir(dir), "elsewhere");
  mkdirSync(target, { recursive: true });
  writeFileSync(join(target, "secret.txt"), "outside secret");

  // Created after makePlugin so the target directory already existsSync.
  symlinkSync(target, join(dir, "cache", "outside"));

  return { dir, target, plugin: makeHost().load(dir) };
}

test("a symlinked directory cannot be read through", () => {
  const { plugin } = escapingPlugin({ fs: { read: "./cache/**" } });
  expect(() =>
    plugin.vm.callFunction("__cap_fs_readText", ["./cache/outside/secret.txt"]),
  ).toThrow(/PermissionDenied: path escapes plugin root/);
});

test("a symlinked directory cannot be written through", () => {
  const { target, plugin } = escapingPlugin({ fs: { write: "./cache/**" } });
  expect(() =>
    plugin.vm.callFunction("__cap_fs_writeText", ["./cache/outside/planted.txt", "x"]),
  ).toThrow(/path escapes plugin root/);
  expect(() => readFileSync(join(target, "planted.txt"), "utf8")).toThrow();
});

test("a symlinked directory cannot be probed with existsSync", () => {
  const { plugin } = escapingPlugin({ fs: { read: "./cache/**" } });
  expect(() =>
    plugin.vm.callFunction("__cap_fs_exists", ["./cache/outside/secret.txt"]),
  ).toThrow(/path escapes plugin root/);
});

test("`*` in the manifest does not lift the symlink boundary", () => {
  const { plugin } = escapingPlugin({ fs: { read: "*" } });
  expect(() =>
    plugin.vm.callFunction("__cap_fs_readText", ["./cache/outside/secret.txt"]),
  ).toThrow(/path escapes plugin root/);
});

test("a symlinked file pointing outside is refused", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: ENTRY,
  });
  const outside = join(outsideDir(dir), "outside.txt");
  writeFileSync(outside, "outside data");
  symlinkSync(outside, join(dir, "link.txt"));

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_readText", ["./link.txt"])).toThrow(
    /path escapes plugin root/,
  );
});

test("a symlink staying inside the plugin root still works", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: ENTRY,
    files: { "assets/real.txt": "real contents" },
  });
  symlinkSync(join(dir, "assets", "real.txt"), join(dir, "link.txt"));

  const plugin = makeHost().load(dir);
  expect(plugin.vm.callFunction("__cap_fs_readText", ["./link.txt"])).toBe("real contents");
});

test("an in-root symlink is matched by its canonical location, not its link path", () => {
  // `./link.txt` really is `./assets/real.txt`, which the grant excludes.
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./link.txt" } }),
    entry: ENTRY,
    files: { "assets/real.txt": "real contents" },
  });
  symlinkSync(join(dir, "assets", "real.txt"), join(dir, "link.txt"));

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_readText", ["./link.txt"])).toThrow(
    /fs.read is not permitted/,
  );
});

test("an entry file symlinked outside the plugin root is refused", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
  });
  const outsideEntry = join(outsideDir(dir), "evil.js");
  writeFileSync(outsideEntry, "export default { onLoad() {} };");
  symlinkSync(outsideEntry, join(dir, "plugin.js"));

  expect(() => makeHost().load(dir)).toThrow(/entry must be a path inside the plugin directory/);
});
