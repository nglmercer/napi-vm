/**
 * `PluginHost` — manifest → permissions → VM → lifecycle.
 *
 * The host owns every decision the guest is not allowed to make: what the
 * manifest may request, where `"./"` points, which capabilities exist in the
 * VM, and when the VM is thrown away and rebuilt.
 */

import * as nodePath from "node:path";

import { Vm } from "../../index";

import { PermissionDeniedError, PluginLoadError } from "./errors";
import { installFsCapability } from "../capabilities/filesystem-capability";
import { createNodeFileSystem, type HostFileSystem } from "../fs/host-filesystem";
import {
  bootstrapSource,
  describePlugin,
  pluginModuleName,
  uninstallLifecycle,
  type PluginContext,
  type UnloadReason,
} from "./lifecycle";
import { parseManifest, type PluginManifest } from "./manifest";
import {
  compilePermissions,
  compilePolicy,
  defaultPolicy,
  FsPermissionChecker,
  type CompiledPermissions,
  type PluginHostPolicy,
} from "./permissions";
import { installPathCapability } from "../capabilities/path-capability";
import { CRYPTO_CAPABILITY } from "../capabilities/crypto-capability";
import { TIMERS_CAPABILITY } from "../capabilities/timers-capability";
import { AUDIO_CAPABILITY } from "../capabilities/audio-capability";
import {
  applyCapabilityOptions,
  defineCapability,
  getCapability,
  hasCapability,
  type CapabilityDefinition,
  type CapabilityTeardown,
} from "../capabilities/capability-registry";
import { FETCH_CAPABILITY } from "../capabilities/fetch-capability";

export const MANIFEST_FILENAME = "plugin.json";

export interface LoadedPlugin {
  manifest: PluginManifest;
  /** Canonical plugin root. Host-side only — never handed to the guest. */
  root: string;
  vm: Vm;
  permissions: CompiledPermissions;
  /** Dynamic capabilities installed in this VM, for teardown. */
  capabilities: string[];
  status: "loaded" | "error";
  error?: Error;
  /** Whatever the last `onLoad` / `onReload` returned. */
  loadResult?: unknown;
}

/**
 * Built-in registry entries. Operators add more with `defineCapability`;
 * everything the host installs — built-in or custom — goes through the same
 * request ∩ grant ∩ kill-switch loop below.
 */
let builtinsRegistered = false;
function ensureBuiltinCapabilities(): void {
  if (builtinsRegistered) return;
  builtinsRegistered = true;
  for (const definition of [
    CRYPTO_CAPABILITY,
    TIMERS_CAPABILITY,
    FETCH_CAPABILITY,
    AUDIO_CAPABILITY,
  ]) {
    if (!hasCapability(definition.name)) defineCapability(definition);
  }
}

/** A plain JSON object (not array/class instance): options-shaped. */
function isOptionsObject(value: unknown): value is Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    Object.getPrototypeOf(value) === Object.prototype
  );
}

export interface PluginHostOptions {
  policy?: PluginHostPolicy;
  /** Swap in a Bun/Deno/Rust-backed filesystem. Defaults to `node:fs`. */
  fs?: HostFileSystem;
}

interface PreparedPlugin {
  manifest: PluginManifest;
  root: string;
  entrySource: string;
  permissions: CompiledPermissions;
  checker: FsPermissionChecker;
}

export class PluginHost {
  private readonly policy: PluginHostPolicy;
  private readonly compiledPolicy: ReturnType<typeof compilePolicy>;
  private readonly fs: HostFileSystem;
  private readonly plugins = new Map<string, LoadedPlugin>();
  /** Capabilities disabled at runtime via {@link setCapabilityEnabled}. */
  private readonly disabled = new Set<string>();
  /**
   * Teardowns returned by each install, per plugin. Kept out of
   * `LoadedPlugin` (public surface) — `dispose` is the only consumer.
   */
  private readonly teardowns = new WeakMap<LoadedPlugin, CapabilityTeardown[]>();

  constructor(options: PluginHostOptions = {}) {
    ensureBuiltinCapabilities();
    this.policy = options.policy ?? defaultPolicy();
    this.compiledPolicy = compilePolicy(this.policy);
    this.fs = options.fs ?? createNodeFileSystem();
  }

  /**
   * Register a capability (trusted-operator API). The definition is what a
   * verified download plus {@link installNativeModule} produces. Throws when
   * the name is taken or malformed.
   */
  defineCapability(definition: CapabilityDefinition): void {
    defineCapability(definition);
  }

  /**
   * Enable or disable a registered capability at runtime. Disabled names are
   * skipped on the next load/reload; already-loaded VMs are untouched (use
   * `reload` to apply). Throws for unknown names.
   */
  setCapabilityEnabled(name: string, enabled: boolean): void {
    if (!hasCapability(name)) {
      throw new PluginLoadError(`unknown capability "${name}"`);
    }
    if (enabled) this.disabled.delete(name);
    else this.disabled.add(name);
  }

  /** Runtime state from {@link setCapabilityEnabled}; granted names start enabled. */
  isCapabilityEnabled(name: string): boolean {
    return hasCapability(name) && !this.disabled.has(name);
  }

  /** Load a plugin directory and run `onLoad`. */
  load(pluginDirectory: string): LoadedPlugin {
    const prepared = this.prepare(pluginDirectory);
    const { name } = prepared.manifest;

    const existing = this.plugins.get(name);
    if (existing && existing.status === "loaded") {
      throw new PluginLoadError(`plugin "${name}" is already loaded`);
    }

    const plugin = this.instantiate(prepared);
    this.plugins.set(name, plugin);
    plugin.loadResult = this.invoke(
      plugin,
      "__plugin_onLoad",
      [this.context(prepared.manifest)],
      "onLoad",
    );
    return plugin;
  }

  /**
   * Rebuild a plugin from disk in a *fresh* VM.
   *
   * The old instance gets `onUnload({ reason: "reload" })`; whatever
   * serializable value it returns is handed to the new instance's `onReload`.
   */
  reload(name: string): LoadedPlugin {
    const current = this.plugins.get(name);
    if (!current) throw new PluginLoadError(`plugin "${name}" is not loaded`);

    let previousState: unknown;
    if (current.status === "loaded") {
      previousState = this.callUnload(current, "reload");
    }
    this.dispose(current);
    this.plugins.delete(name);

    const prepared = this.prepare(current.root);
    if (prepared.manifest.name !== name) {
      throw new PluginLoadError(
        `plugin directory now declares "${prepared.manifest.name}", expected "${name}"`,
      );
    }

    const plugin = this.instantiate(prepared);
    this.plugins.set(name, plugin);
    plugin.loadResult = this.invoke(
      plugin,
      "__plugin_onReload",
      [this.context(prepared.manifest), previousState ?? null],
      "onReload",
    );
    return plugin;
  }

  /** Run `onUnload`, tear the VM down and forget the plugin. */
  unload(name: string): unknown {
    const plugin = this.plugins.get(name);
    if (!plugin) throw new PluginLoadError(`plugin "${name}" is not loaded`);

    let state: unknown;
    let failure: unknown;
    if (plugin.status === "loaded") {
      // A broken `onUnload` must not keep the plugin loaded: tear down first,
      // then report the failure.
      try {
        state = this.callUnload(plugin, "unload");
      } catch (error) {
        failure = error;
      }
    }
    this.dispose(plugin);
    this.plugins.delete(name);
    if (failure) throw failure;
    return state;
  }

  get(name: string): LoadedPlugin | undefined {
    return this.plugins.get(name);
  }

  list(): LoadedPlugin[] {
    return [...this.plugins.values()];
  }

  /** Unload every plugin, ignoring individual failures. */
  unloadAll(): void {
    for (const name of [...this.plugins.keys()]) {
      try {
        this.unload(name);
      } catch {
        this.plugins.delete(name);
      }
    }
  }

  // ── internals ──────────────────────────────────────────────────────

  /** Read + validate the manifest and entry file; compile permissions. */
  private prepare(pluginDirectory: string): PreparedPlugin {
    const resolvedRoot = nodePath.resolve(pluginDirectory);
    const root = this.fs.realpath(resolvedRoot);
    if (root === null) {
      throw new PluginLoadError(`plugin directory not found: ${pluginDirectory}`);
    }

    const manifestPath = nodePath.join(root, MANIFEST_FILENAME);
    if (!this.fs.exists(manifestPath)) {
      throw new PluginLoadError(`missing ${MANIFEST_FILENAME} in ${pluginDirectory}`);
    }
    const manifest = parseManifest(this.fs.readText(manifestPath));
    const permissions = compilePermissions(manifest);

    // The entry file is read by the host, not by the plugin, so it is not
    // subject to `permissions.fs` — but it must still live inside the root,
    // symlinks included.
    const entryNative = nodePath.resolve(
      root,
      manifest.entry.split("/").join(nodePath.sep),
    );
    const entryReal = this.fs.realpath(entryNative);
    if (entryReal === null) {
      throw new PluginLoadError(`entry file not found: ${manifest.entry}`);
    }
    if (entryReal !== root && !entryReal.startsWith(root + nodePath.sep)) {
      throw new PluginLoadError("entry must be a path inside the plugin directory");
    }
    const entrySource = this.fs.readText(entryReal);

    const checker = new FsPermissionChecker(
      root,
      permissions.fs,
      this.compiledPolicy,
      this.fs,
    );

    return { manifest, root, entrySource, permissions, checker };
  }

  /** Build a VM, install capabilities and create the plugin instance. */
  private instantiate(prepared: PreparedPlugin): LoadedPlugin {
    const { manifest, root, entrySource, permissions, checker } = prepared;
    const vm = new Vm();
    const plugin: LoadedPlugin = {
      manifest,
      root,
      vm,
      permissions,
      capabilities: [],
      status: "loaded",
    };

    try {
      // Substrate: always present, not requested. `napi:fs` is installed
      // unconditionally because the checker itself needs no grant to exist —
      // every call through it is still authorized per path.
      const teardowns: CapabilityTeardown[] = [
        installFsCapability(vm, { checker, fs: this.fs }),
      ];
      if (permissions.path) teardowns.push(installPathCapability(vm));
      this.teardowns.set(plugin, teardowns);
      this.installDynamicCapabilities(plugin, checker);

      const moduleName = pluginModuleName(manifest.name);
      vm.registerModule(moduleName, entrySource);
      vm.run(bootstrapSource(moduleName));

      const shape = describePlugin(vm);
      if (!shape.hasInstance) {
        throw new PluginLoadError(
          `plugin "${manifest.name}" must default-export an object or a class`,
        );
      }
    } catch (error) {
      plugin.status = "error";
      plugin.error = asError(error);
      this.dispose(plugin);
      this.plugins.set(manifest.name, plugin);
      throw wrapLoadError(manifest.name, "instantiate", error);
    }

    return plugin;
  }

  private context(manifest: PluginManifest): PluginContext {
    return { name: manifest.name, version: manifest.version };
  }

  private callUnload(plugin: LoadedPlugin, reason: UnloadReason): unknown {
    return this.invoke(
      plugin,
      "__plugin_onUnload",
      [{ ...this.context(plugin.manifest), reason }],
      "onUnload",
    );
  }

  /**
   * Call a lifecycle wrapper, recording failures on the plugin entry.
   *
   * A hook that throws leaves the VM in an unknown state, so its capabilities
   * are revoked immediately — an errored plugin must not keep a live `napi:fs`
   * around waiting for some later cleanup. The registry entry survives (with
   * `status: "error"`) so the plugin can still be reloaded.
   */
  private invoke(
    plugin: LoadedPlugin,
    fn: string,
    args: unknown[],
    hook: string,
  ): unknown {
    try {
      return plugin.vm.callFunction(fn, args);
    } catch (error) {
      plugin.status = "error";
      plugin.error = asError(error);
      this.dispose(plugin);
      throw wrapLoadError(plugin.manifest.name, hook, error);
    }
  }

  /**
   * The policy grant for a capability: the dynamic map first, then the
   * legacy per-capability policy fields. Absent/`false` means denied.
   */
  private grantFor(name: string): unknown {
    const dynamic = this.policy.capabilities?.[name];
    if (dynamic !== undefined) return dynamic;
    switch (name) {
      case "crypto":
        return this.policy.crypto === true ? true : undefined;
      case "timers":
        return this.policy.timers;
      case "fetch":
        return this.policy.fetch;
      default:
        return undefined;
    }
  }

  /**
   * Install every requested capability through one loop: the dynamic
   * `capabilities` map plus the legacy boolean flags (`crypto`, `timers`,
   * `fetch`), which behave as aliases. Every requested name must exist
   * (unknown fails the load); then request ∩ grant ∩ kill-switch decides.
   * Sorted for a deterministic install order. Each install returns its own
   * teardown — there is no per-capability uninstall function.
   */
  private installDynamicCapabilities(plugin: LoadedPlugin, checker: FsPermissionChecker): void {
    const { manifest, vm, permissions } = plugin;
    const requested: Record<string, unknown> = { ...(manifest.permissions?.capabilities ?? {}) };
    if (manifest.permissions?.crypto === true) requested.crypto ??= true;
    if (manifest.permissions?.timers === true) requested.timers ??= true;
    // Mirror the historic gate: an empty origins list installs nothing.
    if (permissions.fetch.any || permissions.fetch.origins.length > 0) {
      requested.fetch ??= manifest.permissions?.fetch;
    }
    const teardowns = this.teardowns.get(plugin) ?? [];
    for (const name of Object.keys(requested).sort()) {
      const options = requested[name];
      if (options === false) continue;
      const definition = getCapability(name);
      if (!definition) {
        throw new PluginLoadError(
          `plugin "${manifest.name}" requests unknown capability "${name}"`,
        );
      }
      const grant = this.grantFor(name);
      if (grant === undefined || grant === false || this.disabled.has(name)) continue;
      if (definition.schema === undefined && isOptionsObject(options) && Object.keys(options).length > 0) {
        throw new PluginLoadError(`capability "${name}" takes no options`);
      }
      const effective =
        definition.schema !== undefined
          ? applyCapabilityOptions(name, definition.schema, options)
          : options;
      teardowns.push(
        definition.install({ vm, manifest, permissions, checker, options: effective, grant }),
      );
      plugin.capabilities.push(name);
    }
  }

  /** Detach every host capability and drop the module graph. */
  private dispose(plugin: LoadedPlugin): void {
    const { vm } = plugin;
    try {
      uninstallLifecycle(vm, pluginModuleName(plugin.manifest.name));
      const teardowns = this.teardowns.get(plugin) ?? [];
      this.teardowns.delete(plugin);
      // Reverse install order: dynamic capabilities first, substrate last.
      for (const teardown of teardowns.reverse()) {
        try {
          teardown();
        } catch {
          // One capability's teardown must not block the rest.
        }
      }
    } catch {
      // A half-built VM is being thrown away anyway.
    }
  }
}

function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function wrapLoadError(name: string, hook: string, error: unknown): Error {
  if (error instanceof PermissionDeniedError) return error;
  const cause = asError(error);
  if (error instanceof PluginLoadError) return error;
  return new PluginLoadError(`plugin "${name}" failed in ${hook}: ${cause.message}`, {
    cause,
  });
}
