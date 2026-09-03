/**
 * Shared fixtures for the plugin-host tests: build a throwaway plugin
 * directory, load it, and clean everything up afterwards.
 */

import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { PluginHost, type PluginHostOptions } from "../../plugins";
import { nodePlatform } from "../../plugins/node";

const roots: string[] = [];

export interface PluginFixture {
  /** Contents of `plugin.json`; objects are stringified. */
  manifest: unknown;
  /** Contents of the entry file (default `plugin.js`). */
  entry?: string;
  /** Extra files, keyed by plugin-relative POSIX path. */
  files?: Record<string, string>;
  /** Symlinks to create, `linkPath` (plugin-relative) → target. */
  symlinks?: Record<string, string>;
  /** Directories to create even when empty. */
  dirs?: string[];
}

/** Materialize a plugin directory under a fresh temp root. */
export function makePlugin(fixture: PluginFixture): string {
  const root = mkdtempSync(join(tmpdir(), "napi-vm-plugin-"));
  roots.push(root);
  const dir = join(root, "plugin");
  mkdirSync(dir, { recursive: true });

  const manifest =
    typeof fixture.manifest === "string"
      ? fixture.manifest
      : JSON.stringify(fixture.manifest, null, 2);
  writeFileSync(join(dir, "plugin.json"), manifest);

  if (fixture.entry !== undefined) {
    writeFileSync(join(dir, "plugin.js"), fixture.entry);
  }

  for (const [relative, contents] of Object.entries(fixture.files ?? {})) {
    const target = join(dir, relative);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, contents);
  }

  for (const relative of fixture.dirs ?? []) {
    mkdirSync(join(dir, relative), { recursive: true });
  }

  for (const [relative, target] of Object.entries(fixture.symlinks ?? {})) {
    const link = join(dir, relative);
    mkdirSync(dirname(link), { recursive: true });
    symlinkSync(target, link);
  }

  return dir;
}

/** The temp root *containing* a plugin dir — handy for "outside" files. */
export function outsideDir(pluginDir: string): string {
  return dirname(pluginDir);
}

const hosts: PluginHost[] = [];

export function makeHost(options: PluginHostOptions = {}): PluginHost {
  // Tests run on Node/Bun: every host gets the Node platform unless the test
  // overrides `platform` (or `fs`, which wins over `platform.fs`).
  const host = new PluginHost({ platform: nodePlatform(), ...options });
  hosts.push(host);
  return host;
}

/** Standard manifest with the given fs permissions. */
export function manifestWith(
  permissions: unknown,
  overrides: Record<string, unknown> = {},
): unknown {
  return {
    name: "test-plugin",
    version: "1.0.0",
    apiVersion: 1,
    entry: "./plugin.js",
    permissions,
    ...overrides,
  };
}

export function cleanup(): void {
  while (hosts.length > 0) hosts.pop()?.unloadAll();
  while (roots.length > 0) {
    const root = roots.pop();
    if (root) rmSync(root, { recursive: true, force: true });
  }
}
