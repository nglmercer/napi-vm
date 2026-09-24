import { test, expect, afterEach } from "bun:test";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cleanup, makeHost, makePlugin, manifestWith } from "./helpers";

afterEach(cleanup);

/** Records hook calls into ./cache/log.txt so the host can read the order. */
const JOURNAL_ENTRY = `
import { readFileSync, writeFileSync, existsSync } from "node:fs";

function append(line) {
  const previous = existsSync("./cache/log.txt") ? readFileSync("./cache/log.txt", "utf8") : "";
  writeFileSync("./cache/log.txt", previous + line + "\\n");
}

export default {
  onLoad(context) {
    append("onLoad:" + context.name + "@" + context.version);
    return "loaded";
  },
  onUnload(context) {
    append("onUnload:" + context.reason);
    return { seen: true };
  }
};
`;

const JOURNAL_PERMISSIONS = { fs: { read: "./cache/**", write: "./cache/**" } };

function journal() {
  const dir = makePlugin({
    manifest: manifestWith(JOURNAL_PERMISSIONS),
    entry: JOURNAL_ENTRY,
    dirs: ["cache"],
  });
  return { dir, host: makeHost() };
}

const log = (dir: string) =>
  readFileSync(join(dir, "cache/log.txt"), "utf8").trim().split("\n");

// ── object and class plugins ─────────────────────────────────────────

test("an object plugin loads", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `export default { onLoad(context) { return "object:" + context.name; } };`,
  });
  expect(makeHost().load(dir).loadResult).toBe("object:test-plugin");
});

test("a class plugin loads and is instantiated once", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `
export default class Counted {
  constructor() { this.calls = 0; }
  onLoad(context) { this.calls = this.calls + 1; return "class:" + context.name; }
  onUnload() { return { calls: this.calls }; }
}
`,
  });
  const host = makeHost();
  expect(host.load(dir).loadResult).toBe("class:test-plugin");
  expect(host.unload("test-plugin")).toEqual({ calls: 1 });
});

test("class instance state survives between hooks", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `
export default class Stateful {
  onLoad() { this.value = 41; }
  onUnload() { return { value: this.value + 1 }; }
}
`,
  });
  const host = makeHost();
  host.load(dir);
  expect(host.unload("test-plugin")).toEqual({ value: 42 });
});

test("missing hooks are optional", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: "export default { };",
  });
  const host = makeHost();
  expect(host.load(dir).status).toBe("loaded");
  expect(host.unload("test-plugin")).toBeUndefined();
});

test("a plugin without a default export is rejected", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: "export function onLoad() {}",
  });
  expect(() => makeHost().load(dir)).toThrow(/must default-export an object or a class/);
});

// ── hook order ───────────────────────────────────────────────────────

test("onLoad runs before onUnload, with the unload reason", () => {
  const { dir, host } = journal();
  host.load(dir);
  host.unload("test-plugin");
  expect(log(dir)).toEqual(["onLoad:test-plugin@1.0.0", "onUnload:unload"]);
});

test("onUnload returns state to the host", () => {
  const { dir, host } = journal();
  host.load(dir);
  expect(host.unload("test-plugin")).toEqual({ seen: true });
});

// ── context ──────────────────────────────────────────────────────────

test("the guest context carries name and version but not the plugin root", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `
export default {
  onLoad(context) {
    const keys = [];
    for (const key in context) { keys.push(key); }
    return keys.sort();
  }
};
`,
  });
  expect(makeHost().load(dir).loadResult).toEqual(["name", "version"]);
});

// ── registry ─────────────────────────────────────────────────────────

test("get and list expose loaded plugins", () => {
  const { dir, host } = journal();
  const plugin = host.load(dir);
  expect(host.get("test-plugin")).toBe(plugin);
  expect(host.list().map((entry) => entry.manifest.name)).toEqual(["test-plugin"]);
  host.unload("test-plugin");
  expect(host.get("test-plugin")).toBeUndefined();
  expect(host.list()).toEqual([]);
});

test("loading the same plugin twice is refused", () => {
  const { dir, host } = journal();
  host.load(dir);
  expect(() => host.load(dir)).toThrow(/already loaded/);
});

test("unloading an unknown plugin is refused", () => {
  expect(() => makeHost().unload("nope")).toThrow(/is not loaded/);
});

test("unload detaches the capabilities from the VM", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" }, path: true }),
    entry: "export default { onLoad() {} };",
  });
  const host = makeHost();
  const plugin = host.load(dir);
  expect(plugin.vm.hasModule("node:fs")).toBe(true);
  expect(plugin.vm.hasModule("node:path")).toBe(true);

  host.unload("test-plugin");
  expect(plugin.vm.hasModule("node:fs")).toBe(false);
  expect(plugin.vm.hasModule("node:path")).toBe(false);
  expect(plugin.vm.hasModule("plugin:test-plugin")).toBe(false);
  expect(plugin.vm.hasGlobal("__cap_fs_readText")).toBe(false);
  expect(plugin.vm.hasGlobal("__cap_path_join")).toBe(false);
});

// ── failures ─────────────────────────────────────────────────────────

test("a throwing onLoad marks the plugin as errored", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `export default { onLoad() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  expect(() => host.load(dir)).toThrow(/failed in onLoad: .*boom/s);
  expect(host.get("test-plugin")?.status).toBe("error");
});

test("a throwing onLoad revokes the capabilities immediately", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" }, path: true }),
    entry: `export default { onLoad() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  expect(() => host.load(dir)).toThrow(/boom/);

  const plugin = host.get("test-plugin");
  expect(plugin?.status).toBe("error");
  const vm = plugin!.vm;
  expect(vm.hasModule("node:fs")).toBe(false);
  expect(vm.hasModule("node:path")).toBe(false);
  expect(vm.hasModule("plugin:test-plugin")).toBe(false);
  expect(vm.hasGlobal("__cap_fs_readText")).toBe(false);
  expect(vm.hasGlobal("__cap_fs_writeText")).toBe(false);
  expect(vm.hasGlobal("__cap_fs_exists")).toBe(false);
  expect(vm.hasGlobal("__cap_path_join")).toBe(false);
  expect(vm.hasGlobal("__plugin_onLoad")).toBe(false);
});

test("a throwing onReload revokes the new VM's capabilities", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" }, path: true }),
    entry: `export default { onLoad() {}, onReload() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  const first = host.load(dir);
  expect(() => host.reload("test-plugin")).toThrow(/boom/);

  const plugin = host.get("test-plugin");
  expect(plugin?.status).toBe("error");
  expect(plugin?.vm).not.toBe(first.vm);
  expect(plugin!.vm.hasModule("node:fs")).toBe(false);
  expect(plugin!.vm.hasGlobal("__cap_fs_readText")).toBe(false);
  // The VM replaced by the reload is gone too.
  expect(first.vm.hasModule("node:fs")).toBe(false);
});

test("a throwing onUnload still unloads the plugin", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "./**" } }),
    entry: `export default { onLoad() {}, onUnload() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  const plugin = host.load(dir);
  expect(() => host.unload("test-plugin")).toThrow(/boom/);

  expect(host.get("test-plugin")).toBeUndefined();
  expect(plugin.vm.hasModule("node:fs")).toBe(false);
  expect(plugin.vm.hasGlobal("__cap_fs_readText")).toBe(false);
});

test("an errored plugin can still be reloaded after a fix", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `export default { onLoad() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  expect(() => host.load(dir)).toThrow(/boom/);

  writeFileSync(join(dir, "plugin.js"), `export default { onReload() { return "fixed"; } };`);
  expect(host.reload("test-plugin").loadResult).toBe("fixed");
  expect(host.get("test-plugin")?.vm.hasModule("node:fs")).toBe(true);
});

test("a directory without plugin.json is refused", () => {
  const dir = makePlugin({ manifest: "{}", entry: "export default {};" });
  rmSync(join(dir, "plugin.json"));
  expect(() => makeHost().load(dir)).toThrow(/missing plugin.json/);
});

test("a missing plugin directory is refused", () => {
  expect(() => makeHost().load("/definitely/not/here")).toThrow(/plugin directory not found/);
});

test("a missing entry file is refused", () => {
  const dir = makePlugin({ manifest: manifestWith({}) });
  expect(() => makeHost().load(dir)).toThrow(/entry file not found/);
});

// ── capabilities are opt-in ──────────────────────────────────────────

test("node:path is only registered when the manifest asks for it", () => {
  const withPath = makePlugin({
    manifest: manifestWith({ path: true }),
    entry: `
import { join, basename, dirname, extname, normalize } from "node:path";
export default {
  onLoad() {
    return [
      join("cache", "foo.json"),
      normalize("a/./b/../c"),
      dirname("a/b/c.txt"),
      basename("a/b/c.txt"),
      extname("a/b/c.txt")
    ];
  }
};
`,
  });
  expect(makeHost().load(withPath).loadResult).toEqual([
    "cache/foo.json",
    "a/c",
    "a/b",
    "c.txt",
    ".txt",
  ]);

  const withoutPath = makePlugin({
    manifest: manifestWith({}),
    entry: `import { join } from "node:path";\nexport default { onLoad() { return join("a", "b"); } };`,
  });
  expect(() => makeHost().load(withoutPath)).toThrow();
});

test("the sandbox is intact inside a plugin", () => {
  const dir = makePlugin({
    manifest: manifestWith({ fs: { read: "*", write: "*" }, path: true }),
    entry: `
export default {
  onLoad() {
    let requireThrew = false;
    try { require("node:fs"); } catch (error) { requireThrew = true; }
    return {
      // The VM ships inert stubs for these; they are empty objects, never
      // Node's real bindings.
      requireStub: JSON.stringify(require),
      processStub: JSON.stringify(process),
      requireThrew: requireThrew,
      bun: typeof Bun,
      deno: typeof Deno,
      bridge: typeof globalThis.__cap_fs_readText
    };
  }
};
`,
  });
  expect(makeHost().load(dir).loadResult).toEqual({
    requireStub: "{}",
    processStub: "{}",
    requireThrew: true,
    bun: "undefined",
    deno: "undefined",
    // The capability bridge is a plain global: it is guarded by the permission
    // checks behind it, not by being hidden.
    bridge: "function",
  });
});
