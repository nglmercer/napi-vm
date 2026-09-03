import { test, expect, afterEach } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cleanup, makeHost, makePlugin, manifestWith, outsideDir } from "./helpers";

afterEach(cleanup);

const readEntry = (path: string) => `
import { readText } from "napi:fs";
export default {
  onLoad() {
    return readText(${JSON.stringify(path)});
  }
};
`;

const writeEntry = (path: string, contents: string) => `
import { writeText } from "napi:fs";
export default {
  onLoad() {
    return writeText(${JSON.stringify(path)}, ${JSON.stringify(contents)});
  }
};
`;

// ── relative reads ───────────────────────────────────────────────────

test("relative read allowed by ./**", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: readEntry("./config.json"),
    files: { "config.json": '{"ok":true}' },
  });
  expect(makeHost().load(dir).loadResult).toBe('{"ok":true}');
});

test("relative read denied outside the granted subtree", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./assets/**" } }),
    entry: readEntry("./config.json"),
    files: { "config.json": "secret", "assets/a.txt": "asset" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied: fs.read is not permitted/);
});

test("a granted subtree is still readable", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./assets/**" } }),
    entry: readEntry("./assets/icons/a.txt"),
    files: { "assets/icons/a.txt": "icon" },
  });
  expect(makeHost().load(dir).loadResult).toBe("icon");
});

test("a single-file grant matches only that file", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./config.json" } }),
    entry: readEntry("./secret.txt"),
    files: { "config.json": "{}", "secret.txt": "s3cret" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied/);
});

test("paths without a leading ./ resolve against the plugin root too", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./config.json" } }),
    entry: readEntry("config.json"),
    files: { "config.json": "plain" },
  });
  expect(makeHost().load(dir).loadResult).toBe("plain");
});

test("\"./\" is the plugin root, not the process working directory", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./package.json" } }),
    entry: readEntry("./package.json"),
    files: { "package.json": '{"iam":"the plugin copy"}' },
  });
  expect(makeHost().load(dir).loadResult).toBe('{"iam":"the plugin copy"}');
});

// ── writes ───────────────────────────────────────────────────────────

test("write allowed inside ./cache/**", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { write: "./cache/**" } }),
    entry: writeEntry("./cache/test.json", '{"written":true}'),
    dirs: ["cache"],
  });
  expect(makeHost().load(dir).loadResult).toBe(true);
  expect(readFileSync(join(dir, "cache/test.json"), "utf8")).toBe('{"written":true}');
});

test("write denied outside the granted subtree", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { write: "./cache/**" } }),
    entry: writeEntry("./config.json", "overwritten"),
    files: { "config.json": "original" },
    dirs: ["cache"],
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied: fs.write is not permitted/);
  expect(readFileSync(join(dir, "config.json"), "utf8")).toBe("original");
});

test("read permission does not imply write permission", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: writeEntry("./cache/test.json", "x"),
    dirs: ["cache"],
  });
  expect(() => makeHost().load(dir)).toThrow(/fs.write is not permitted/);
});

test("writeText rejects non-string contents", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { write: "./**" } }),
    entry: `
import { writeText } from "napi:fs";
export default { onLoad() { return writeText("./a.txt", 42); } };
`,
  });
  expect(() => makeHost().load(dir)).toThrow(/contents must be a string/);
});

// ── exists ───────────────────────────────────────────────────────────

test("exists reports presence for permitted paths", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: `
import { exists } from "napi:fs";
export default {
  onLoad() { return [exists("./config.json"), exists("./nope.json")]; }
};
`,
    files: { "config.json": "{}" },
  });
  expect(makeHost().load(dir).loadResult).toEqual([true, false]);
});

test("exists is itself permission-checked", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./assets/**" } }),
    entry: `
import { exists } from "napi:fs";
export default { onLoad() { return exists("./config.json"); } };
`,
    files: { "config.json": "{}" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied/);
});

// ── missing / false permissions ──────────────────────────────────────

test("empty permissions deny every read", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: readEntry("./config.json"),
    files: { "config.json": "{}" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied/);
});

test("a manifest without a permissions block denies every read", () => {
  const dir = makePlugin({
    manifest: {
      name: "test-plugin",
      version: "1.0.0",
      apiVersion: 1,
      entry: "./plugin.js",
    },
    entry: readEntry("./config.json"),
    files: { "config.json": "{}" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied/);
});

test("read: false denies", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: false } }),
    entry: readEntry("./config.json"),
    files: { "config.json": "{}" },
  });
  expect(() => makeHost().load(dir)).toThrow(/PermissionDenied/);
});

test("read: true behaves like \"*\" and is bounded by the plugin root", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: true } }),
    entry: readEntry("./config.json"),
    files: { "config.json": "anything" },
  });
  expect(makeHost().load(dir).loadResult).toBe("anything");
});

// ── confinement: no outside-root access, ever ─────────────────────────

test("absolute read is confined to the plugin root", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "/**" } }),
    entry: "export default { onLoad() {} };",
  });
  const outside = join(outsideDir(dir), "outside.txt");
  writeFileSync(outside, "outside data");

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [outside])).toThrow(
    /absolute filesystem reads are outside the plugin root/,
  );
});

test("an absolute manifest grant cannot reach outside the root", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "/**" } }),
    entry: "export default { onLoad() {} };",
  });
  const outside = join(outsideDir(dir), "outside.txt");
  writeFileSync(outside, "outside data");

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [outside])).toThrow(
    /outside the plugin root/,
  );
  expect(readFileSync(outside, "utf8")).toBe("outside data");
});

test("absolute read denied even when the manifest grants a subtree", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: "export default { onLoad() {} };",
  });
  const outside = join(outsideDir(dir), "outside.txt");
  writeFileSync(outside, "outside data");

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_readText", [outside])).toThrow(
    /absolute filesystem reads are outside the plugin root/,
  );
});

test("absolute write denied even when the manifest asks for \"*\"", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "*", write: "*" } }),
    entry: "export default { onLoad() {} };",
  });
  const outside = join(outsideDir(dir), "outside.txt");
  writeFileSync(outside, "original");

  const plugin = makeHost().load(dir);
  expect(() => plugin.vm.callFunction("__cap_fs_writeText", [outside, "hacked"])).toThrow(
    /absolute filesystem writes are outside the plugin root/,
  );
  expect(readFileSync(outside, "utf8")).toBe("original");
});

test("\"*\" inside the plugin root works with the default policy", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "*" } }),
    entry: readEntry("./config.json"),
    files: { "config.json": "in-root" },
  });
  expect(makeHost().load(dir).loadResult).toBe("in-root");
});

// ── error hygiene ────────────────────────────────────────────────────

test("permission errors do not leak host paths", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./assets/**" } }),
    entry: readEntry("./secret.txt"),
    files: { "secret.txt": "s3cret" },
  });
  try {
    makeHost().load(dir);
    throw new Error("expected a permission failure");
  } catch (error) {
    const message = (error as Error).message;
    expect(message).toMatch(/PermissionDenied/);
    expect(message).not.toContain(dir);
  }
});

test("a guest sees a typed PermissionDenied error it can catch", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./assets/**" } }),
    entry: `
import { readText } from "napi:fs";
export default {
  onLoad() {
    try {
      readText("./secret.txt");
      return "unexpectedly allowed";
    } catch (error) {
      return error.name + " | " + error.message;
    }
  }
};
`,
    files: { "secret.txt": "s3cret" },
  });
  expect(makeHost().load(dir).loadResult).toBe(
    'PermissionDenied | fs.read is not permitted for "./secret.txt"',
  );
});
