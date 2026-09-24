import { test, expect, afterEach } from "bun:test";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { cleanup, makeHost, makePlugin, manifestWith } from "./helpers";

afterEach(cleanup);

const JOURNAL_ENTRY = `
import { readFileSync, writeFileSync, existsSync } from "node:fs";

function append(line) {
  const previous = existsSync("./cache/log.txt") ? readFileSync("./cache/log.txt", "utf8") : "";
  writeFileSync("./cache/log.txt", previous + line + "\\n");
}

export default class Journal {
  onLoad(context) {
    append("onLoad:" + context.name);
    this.generation = 1;
    return this.generation;
  }
  onUnload(context) {
    append("onUnload:" + context.reason);
    return { generation: this.generation };
  }
  onReload(context, previousState) {
    append("onReload:" + (previousState ? previousState.generation : "none"));
    this.generation = (previousState ? previousState.generation : 0) + 1;
    return this.generation;
  }
}
`;

const PERMISSIONS = { fs: { read: "./**", write: "./cache/**" } };

function journal() {
  const dir = makePlugin({
    manifest: manifestWith(PERMISSIONS),
    entry: JOURNAL_ENTRY,
    dirs: ["cache"],
  });
  return { dir, host: makeHost() };
}

const log = (dir: string) =>
  readFileSync(join(dir, "cache/log.txt"), "utf8").trim().split("\n");

test("reload runs onUnload(reason=reload) then onReload with the previous state", () => {
  const { dir, host } = journal();
  host.load(dir);
  const reloaded = host.reload("test-plugin");

  expect(log(dir)).toEqual(["onLoad:test-plugin", "onUnload:reload", "onReload:1"]);
  expect(reloaded.loadResult).toBe(2);
});

test("reload builds a brand-new VM", () => {
  const { dir, host } = journal();
  const first = host.load(dir);
  const second = host.reload("test-plugin");
  expect(second.vm).not.toBe(first.vm);
  expect(first.vm.hasModule("plugin:test-plugin")).toBe(false);
  expect(second.vm.hasModule("plugin:test-plugin")).toBe(true);
});

test("reload picks up edited source and permissions", () => {
  const { dir, host } = journal();
  host.load(dir);

  writeFileSync(
    join(dir, "plugin.js"),
    `
import { readFileSync } from "node:fs";
export default {
  onLoad() { return "v2:" + readFileSync("./config.json", "utf8"); },
  onReload() { return "v2:" + readFileSync("./config.json", "utf8"); }
};
`,
  );
  writeFileSync(join(dir, "config.json"), "fresh");

  expect(host.reload("test-plugin").loadResult).toBe("v2:fresh");
});

test("reload re-reads the manifest, so tightened permissions take effect", () => {
  const { dir, host } = journal();
  host.load(dir);

  writeFileSync(
    join(dir, "plugin.js"),
    `
import { readFileSync } from "node:fs";
export default {
  onLoad() { return readFileSync("./config.json", "utf8"); },
  onReload() { return readFileSync("./config.json", "utf8"); }
};
`,
  );
  writeFileSync(join(dir, "config.json"), "secret");
  writeFileSync(
    join(dir, "plugin.json"),
    JSON.stringify(manifestWith({ fs: { read: "./cache/**", write: "./cache/**" } })),
  );

  expect(() => host.reload("test-plugin")).toThrow(/fs.read is not permitted/);
});

test("onReload falls back to onLoad when the plugin has no onReload", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `
export default {
  onLoad(context) { return "loaded:" + context.name; },
  onUnload() { return { keep: 1 }; }
};
`,
  });
  const host = makeHost();
  host.load(dir);
  expect(host.reload("test-plugin").loadResult).toBe("loaded:test-plugin");
});

test("only serializable state crosses between VMs", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `
export default {
  onLoad() {},
  onUnload() { return { n: 7, list: [1, 2], nested: { ok: true } }; },
  onReload(context, previousState) { return previousState; }
};
`,
  });
  const host = makeHost();
  host.load(dir);
  expect(host.reload("test-plugin").loadResult).toEqual({
    n: 7,
    list: [1, 2],
    nested: { ok: true },
  });
});

test("reload of an unknown plugin is refused", () => {
  expect(() => makeHost().reload("nope")).toThrow(/is not loaded/);
});

test("reload refuses a directory that renamed itself", () => {
  const { dir, host } = journal();
  host.load(dir);
  writeFileSync(
    join(dir, "plugin.json"),
    JSON.stringify(manifestWith(PERMISSIONS, { name: "other-plugin" })),
  );
  expect(() => host.reload("test-plugin")).toThrow(/now declares "other-plugin"/);
});

test("a plugin that failed to load can be reloaded after a fix", () => {
  const dir = makePlugin({
    manifest: manifestWith({}),
    entry: `export default { onLoad() { throw new Error("boom"); } };`,
  });
  const host = makeHost();
  expect(() => host.load(dir)).toThrow(/boom/);
  expect(host.get("test-plugin")?.status).toBe("error");

  writeFileSync(join(dir, "plugin.js"), `export default { onReload() { return "fixed"; } };`);
  expect(host.reload("test-plugin").loadResult).toBe("fixed");
  expect(host.get("test-plugin")?.status).toBe("loaded");
});
