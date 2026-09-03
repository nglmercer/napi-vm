/**
 * Loading a plugin with `PluginHost`.
 *
 * The plugin in `examples/plugins/example-plugin` declares its filesystem
 * permissions in `plugin.json`; the host intersects them with its own policy
 * and installs `napi:fs` / `napi:path` into a sealed VM.
 *
 * Run:  bun examples/plugins.ts
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { PluginHost, type PluginHostPolicy } from "../plugins";
import { nodePlatform } from "../plugins/node";

const PLUGIN_DIR = join(import.meta.dir, "plugins/example-plugin");

// The host is the final authority: plugins are confined to their own
// directory, and every capability needs an explicit grant here.
const policy: PluginHostPolicy = {
  capabilities: {},
};

const host = new PluginHost({ policy, platform: nodePlatform() });

// ── load ────────────────────────────────────────────────────────────

const plugin = host.load(PLUGIN_DIR);
console.log(`loaded ${plugin.manifest.name}@${plugin.manifest.version}`);
console.log("onLoad returned:", plugin.loadResult);
console.log(
  "cache/status.json:",
  readFileSync(join(PLUGIN_DIR, "cache/status.json"), "utf8"),
);

// ── what the plugin cannot do ───────────────────────────────────────
// `plugin.json` grants reads of ./config.json and ./assets/** only, so every
// one of these is refused *after* the path is canonicalized.

for (const attempt of [
  "./plugin.json", // outside the granted patterns
  "./assets/../plugin.json", // traversal, normalized first
  "../../../etc/passwd", // escapes the plugin root
  "/etc/passwd", // absolute, outside the plugin root
]) {
  try {
    plugin.vm.callFunction("__cap_fs_readText", [attempt]);
    console.log(`  ${attempt} -> UNEXPECTEDLY ALLOWED`);
  } catch (error) {
    console.log(`  ${attempt} -> ${(error as Error).message}`);
  }
}

// ── reload ──────────────────────────────────────────────────────────
// The old VM is discarded; state travels as a plain serializable value.

const reloaded = host.reload("example-plugin");
console.log("onReload returned:", reloaded.loadResult);
console.log("fresh VM:", reloaded.vm !== plugin.vm);

// ── unload ──────────────────────────────────────────────────────────

const state = host.unload("example-plugin");
console.log("onUnload returned:", JSON.stringify(state));
console.log("still loaded:", host.list().length);
