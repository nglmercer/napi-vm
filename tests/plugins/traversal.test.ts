import { test, expect, afterEach } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cleanup, makeHost, makePlugin, manifestWith, outsideDir } from "./helpers";

afterEach(cleanup);

/** A plugin that reads whatever path the host hands it at call time. */
const PROBE_ENTRY = `
import { readText, writeText, exists } from "napi:fs";
export default {
  onLoad() {},
  read(path) { return readText(path); },
  write(path, contents) { return writeText(path, contents); },
  check(path) { return exists(path); }
};
`;

function probe(permissions: unknown) {
  const dir = makePlugin({
    manifest: manifestWith(permissions),
    entry: `
${PROBE_ENTRY}
`,
    files: { "config.json": "{}", "secret.txt": "s3cret", "cache/keep.txt": "keep" },
  });
  writeFileSync(join(outsideDir(dir), "outside.txt"), "outside data");
  const plugin = makeHost().load(dir);
  return { dir, plugin };
}

function read(pluginVmPath: string, permissions: unknown = { fs: { read: "./cache/**" } }) {
  const { plugin } = probe(permissions);
  return () => plugin.vm.callFunction("__cap_fs_readText", [pluginVmPath]);
}

test("`..` climbing out of the plugin root is refused", () => {
  expect(read("./cache/../../outside.txt")).toThrow(/PermissionDenied: path escapes plugin root/);
});

test("a bare `..` prefix is refused", () => {
  expect(read("../outside.txt")).toThrow(/path escapes plugin root/);
});

test("`..` inside the root is normalized, then permission-checked", () => {
  // Resolves to ./secret.txt, which the ./cache/** grant does not cover.
  expect(read("./cache/../secret.txt")).toThrow(/fs.read is not permitted/);
});

test("`..` that resolves back into the granted subtree is allowed", () => {
  const { plugin } = probe({ fs: { read: "./cache/**" } });
  expect(plugin.vm.callFunction("__cap_fs_readText", ["./cache/sub/../keep.txt"])).toBe("keep");
});

test("Windows-style separators are folded before the check", () => {
  expect(read("..\\outside.txt")).toThrow(/path escapes plugin root/);
  expect(read(".\\cache\\..\\..\\outside.txt")).toThrow(/path escapes plugin root/);
});

test("a deep `..` chain is refused", () => {
  expect(read("./cache/../../../../../../etc/passwd")).toThrow(/path escapes plugin root/);
});

test("traversal is refused for writes too", () => {
  const { dir, plugin } = probe({ fs: { write: "./cache/**" } });
  const outside = join(outsideDir(dir), "outside.txt");
  expect(() =>
    plugin.vm.callFunction("__cap_fs_writeText", ["./cache/../../outside.txt", "hacked"]),
  ).toThrow(/path escapes plugin root/);
  expect(readFileSync(outside, "utf8")).toBe("outside data");
});

test("traversal is refused for exists too", () => {
  const { plugin } = probe({ fs: { read: "./cache/**" } });
  expect(() => plugin.vm.callFunction("__cap_fs_exists", ["../outside.txt"])).toThrow(
    /path escapes plugin root/,
  );
});

test("a NUL byte in a path is refused", () => {
  expect(read("./cache/keep.txt\0.png")).toThrow(/NUL byte/);
});

test("a non-string path is refused", () => {
  const { plugin } = probe({ fs: { read: "*" } });
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [42])).toThrow(
    /requires a non-empty path string/,
  );
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [""])).toThrow(
    /requires a non-empty path string/,
  );
});

test("an absolute path is not a traversal error but a confinement error", () => {
  const { dir, plugin } = probe({ fs: { read: "*" } });
  const outside = join(outsideDir(dir), "outside.txt");
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [outside])).toThrow(
    /outside the plugin root/,
  );
});
