/**
 * `PluginHost` — manifest → permissions → VM → lifecycle.
 *
 * The host owns every decision the guest is not allowed to make: what the
 * manifest may request, where `"./"` points, which capabilities exist in the
 * VM, and when the VM is thrown away and rebuilt.
 *
 * The host is portable: every outside-world access (filesystem, paths,
 * crypto, native loading) goes through the injected {@link HostPlatform}.
 * Node/Bun hosts pass `platform: nodePlatform()` from
 * `"napi-vm/plugins/node"`; other runtimes assemble
 * `portablePlatform(theirFileSystem)`.
 */

import { Vm } from "../../index";

import { PermissionDeniedError, PluginLoadError } from "./errors";
import { installFsCapability } from "../capabilities/filesystem-capability";
import { validateEntryPath } from "../fs/path-rules";
import {
  missingFileSystem,
  portablePlatform,
  type HostFileSystem,
  type HostPlatform,
} from "../platform";
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
  type CapabilityGrant,
  type CompiledPermissions,
} from "./permissions";
import { FsPermissionChecker, type CompiledFsPermissions } from "../fs/checker";
import { PATH_CAPABILITY } from "../capabilities/path-capability";
import { CRYPTO_CAPABILITY } from "../capabilities/crypto-capability";
import { TIMERS_CAPABILITY } from "../capabilities/timers-capability";
import { AUDIO_CAPABILITY } from "../capabilities/audio-capability";
import {
  applyCapabilityOptions,
  defineCapability,
  getCapability,
  hasCapability,
  isPermissionGranted,
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
    PATH_CAPABILITY,
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

export interface PluginHostPolicy {
  /**
   * Per-capability grants keyed by CAPABILITY name (post-resolve):
   * `{ "<name>": true }` or `{ "<name>": {...} }`. Absent, null or `false`
   * means denied. Unknown names are inert until defined.
   */
  capabilities?: Record<string, CapabilityGrant>;
}

/**
 * The default policy: no grants at all. Confinement to the plugin's own
 * directory is enforced unconditionally by the checker itself.
 */
export function defaultPolicy(): PluginHostPolicy {
  return { capabilities: {} };
}

export interface PluginHostOptions {
  policy?: PluginHostPolicy;
  /**
   * The outside world: filesystem, paths, crypto, native loader. Node/Bun
   * hosts pass `nodePlatform()` from `"napi-vm/plugins/node"`; portable
   * hosts pass `portablePlatform(theirFileSystem)`.
   */
  platform?: HostPlatform;
  /**
   * Filesystem override, winning over `platform.fs`. Handy for tests and for
   * hosts that only need to swap the backend (e.g. byte limits) while
   * keeping the platform's paths and crypto.
   */
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
  private readonly platform: HostPlatform;
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
    const base = options.platform ?? portablePlatform(missingFileSystem());
    this.platform = options.fs ? { ...base, fs: options.fs } : base;
  }

  /**
   * Register a capability (trusted-operator API). The definition is what a
   * verified download plus the native bridge produces. Throws when the name
   * is taken or malformed.
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
    const { fs, path } = this.platform;
    const resolvedRoot = path.resolve(pluginDirectory);
    const root = fs.realpath(resolvedRoot);
    if (root === null) {
      throw new PluginLoadError(`plugin directory not found: ${pluginDirectory}`);
    }

    const manifestPath = path.join(root, MANIFEST_FILENAME);
    if (!fs.exists(manifestPath)) {
      throw new PluginLoadError(`missing ${MANIFEST_FILENAME} in ${pluginDirectory}`);
    }
    const manifest = parseManifest(fs.readText(manifestPath));
    // Entry containment is enforced textually here and canonically below
    // (`realpath`); the manifest itself carries the raw string.
    manifest.entry = validateEntryPath(manifest.entry);
    const permissions = compilePermissions(manifest);

    // The entry file is read by the host, not by the plugin, so it is not
    // subject to `permissions.fs` — but it must still live inside the root,
    // symlinks included.
    const entryNative = path.resolve(root, manifest.entry.split("/").join(path.sep));
    const entryReal = fs.realpath(entryNative);
    if (entryReal === null) {
      throw new PluginLoadError(`entry file not found: ${manifest.entry}`);
    }
    if (entryReal !== root && !entryReal.startsWith(root + path.sep)) {
      throw new PluginLoadError("entry must be a path inside the plugin directory");
    }
    const entrySource = fs.readText(entryReal);

    // No `fs` request means no rules: default-deny, never undefined.
    const fsRules = (permissions.fs ?? { read: [], write: [] }) as CompiledFsPermissions;
    const checker = new FsPermissionChecker(root, fsRules, fs, path);

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
      // Substrate: `napi:fs` is installed unconditionally because the
      // checker itself needs no grant to exist — every call through it is
      // still authorized per path. Everything else, including `napi:path`,
      // flows through the capability loop below.
      const teardowns: CapabilityTeardown[] = [
        installFsCapability(vm, { checker, fs: this.platform.fs }),
      ];
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
   * Install every requested permission through one loop — no per-capability
   * branches. A manifest key WITH `resolve` is infrastructure (an expander
   * like `fs` or `capabilities`): it expands into capability installs and is
   * never installed itself. A key WITHOUT it installs itself directly.
   * Each expanded capability resolves against the registry (unknown fails
   * the load), and the key's `allows` decides request ∩ grant. The runtime
   * kill-switch (`setCapabilityEnabled`) applies last. Sorted for a
   * deterministic install order. Each install returns its own teardown.
   */
  private installDynamicCapabilities(plugin: LoadedPlugin, checker: FsPermissionChecker): void {
    const { manifest, vm, permissions } = plugin;
    const requested = (manifest.permissions ?? {}) as Record<string, unknown>;
    const teardowns = this.teardowns.get(plugin) ?? [];
    for (const name of Object.keys(requested).sort()) {
      const options = requested[name];
      if (options === false) continue;
      const key = getCapability(name);
      if (!key) {
        throw new PluginLoadError(
          `plugin "${manifest.name}" has no permission binding for "${name}"`,
        );
      }
      const resolved = key.resolve ? key.resolve(options) : [{ capability: name, options }];
      for (const { capability, options: capOptions } of resolved) {
        if (capOptions === false) continue;
        const definition = getCapability(capability);
        if (!definition) {
          throw new PluginLoadError(
            `plugin "${manifest.name}" requests unknown capability "${capability}"`,
          );
        }
        if (this.disabled.has(capability)) continue;
        const grant = this.policy.capabilities?.[capability];
        const allows = key.allows
          ? key.allows(capOptions, grant)
          : isPermissionGranted(grant);
        if (!allows) continue;
        if (definition.schema === undefined && isOptionsObject(capOptions) && Object.keys(capOptions).length > 0) {
          throw new PluginLoadError(`capability "${capability}" takes no options`);
        }
        const effective =
          definition.schema !== undefined
            ? applyCapabilityOptions(capability, definition.schema, capOptions)
            : capOptions;
        if (typeof definition.install !== "function") {
          throw new PluginLoadError(
            `plugin "${manifest.name}" requests "${capability}", which cannot be installed directly`,
          );
        }
        teardowns.push(
          definition.install({
            vm,
            manifest,
            permissions,
            checker,
            platform: this.platform,
            options: effective,
            grant,
          }),
        );
        plugin.capabilities.push(capability);
      }
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
