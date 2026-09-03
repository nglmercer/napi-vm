/**
 * Capability registry: names are data, not syntax — and every capability
 * shares one lifecycle.
 *
 * A capability is a small subplugin declared in ONE place. It validates its
 * own manifest value, decides its own install shape, and wires guest modules
 * in `install`, returning its own teardown. There is no per-capability
 * uninstall function; the host runs the returned teardowns on unload, reload
 * and failed hooks.
 *
 * ```ts
 * defineCapability({
 *   name: "greet",
 *   schema: { voice: { type: "string", default: "alto", enum: ["alto", "bass"] } },
 *   install: ({ vm, options }) => {
 *     const globals = vm.registerHostModule("napi:greet", { ... });
 *     return () => unbindCapabilityModule(vm, "napi:greet", globals);
 *   },
 * });
 * ```
 *
 * Manifest requests (`capabilities: { "<name>": true | {...} }`) resolve
 * against this registry at load time: unknown names fail the load, and
 * requested-but-ungranted names stay absent.
 */

import type { Vm } from "../../index";

import { PluginLoadError, PluginManifestError } from "../core/errors";
import type { CompiledPermissions } from "../core/permissions";
import type { FsPermissionChecker } from "../fs/checker";
import type { PluginManifest } from "../core/manifest";
import type { HostPlatform } from "../platform";

/**
 * One dynamic capability request: `true` for defaults, or an options object
 * the capability's installer validates. Names resolve against the host's
 * capability registry at load time — an unknown name fails the load.
 */
export type CapabilityRequest = boolean | Record<string, unknown>;

/** What an installed capability may use. */
export interface CapabilityContext {
  vm: Vm;
  manifest: PluginManifest;
  permissions: CompiledPermissions;
  /**
   * The loading plugin's path checker. Optional because most capabilities
   * never touch a path; definitions that expose a path sink fail closed
   * without one (see `core/native-bridge.ts`).
   */
  checker?: FsPermissionChecker;
  /** The host platform (filesystem, paths, crypto, native loader). */
  platform: HostPlatform;
  /** Manifest-side value after schema defaults: `{}` when requested as `true`. */
  options: unknown;
  /** Policy-side value: `true` or the granted options object. */
  grant: unknown;
}

/**
 * Cleanup returned by `install`: removes the guest module and every bridge
 * global the install created. The host runs teardowns in reverse install
 * order; one throwing must not block the rest (the host guards that).
 */
export type CapabilityTeardown = () => void;

/** One expanded install: which capability, with which options. */
export interface PermissionResolved {
  capability: string;
  options: unknown;
}

export interface CapabilityDefinition {
  /** Registry name, e.g. `"audio"`. Validated, not free-form. */
  readonly name: string;
  /**
   * Validate and normalize the manifest's `permissions.<name>` value. Throws
   * `PluginManifestError` on anything the key does not accept; absent means
   * the raw value passes through untouched.
   */
  validate?(value: unknown, field: string): unknown;
  /**
   * Expand a manifest request into capability installs. Default: one install
   * of this same name with the request as options.
   *
   * A definition WITH `resolve` is infrastructure (an expander like `fs` or
   * `capabilities`): the host runs the expansion and never calls `install`.
   * A definition WITHOUT it installs itself through `install`.
   */
  resolve?(request: unknown): PermissionResolved[];
  /**
   * Build the installer-facing form stored in the compiled permission map.
   * Default: the request as-is.
   */
  compile?(request: unknown): unknown;
  /**
   * The request ∩ grant decision. Default: granted unless the grant is
   * absent, null, or `false`.
   */
  allows?(request: unknown, grant: unknown): boolean;
  /**
   * Manifest-side options, validated with defaults applied before `install`
   * runs. Absent means "no options": `true` installs with `{}`, and any
   * options object is refused.
   */
  readonly schema?: CapabilityOptionsSchema;
  /**
   * Wire the guest module; return its teardown. Absent only on expander
   * entries (`fs`, `capabilities`), which the host never installs directly.
   */
  install?(ctx: CapabilityContext): CapabilityTeardown;
}

/** Remove a guest module and its bridge globals — the shared teardown. */
export function unbindCapabilityModule(
  vm: Vm,
  moduleName: string,
  globals: readonly string[],
): void {
  vm.removeModule(moduleName);
  for (const name of globals) vm.removeGlobal(name);
}

/** Default install gate: denied when the grant is absent, null, or `false`. */
export function isPermissionGranted(grant: unknown): boolean {
  return grant !== undefined && grant !== null && grant !== false;
}

/** Shared manifest validator for boolean flags (`path`, `crypto`, …). */
export function booleanPermissionValue(value: unknown, field: string): boolean {
  if (typeof value !== "boolean") {
    throw new PluginManifestError(`${field} must be a boolean`);
  }
  return value;
}

// ── option schemas (no dependency; deliberately small) ─────────────

export type CapabilityFieldSchema =
  | { type: "boolean"; default?: boolean }
  | { type: "number"; default?: number; min?: number; max?: number; integer?: boolean }
  | { type: "string"; default?: string; enum?: readonly string[] }
  | { type: "stringArray"; default?: string[] };

export type CapabilityOptionsSchema = Record<string, CapabilityFieldSchema>;

function checkField(
  capability: string,
  key: string,
  field: CapabilityFieldSchema,
  value: unknown,
): unknown {
  const where = `capability "${capability}": option "${key}"`;
  switch (field.type) {
    case "boolean":
      if (typeof value !== "boolean") throw new PluginLoadError(`${where} must be a boolean`);
      return value;
    case "number": {
      if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new PluginLoadError(`${where} must be a finite number`);
      }
      if (field.integer === true && !Number.isInteger(value)) {
        throw new PluginLoadError(`${where} must be an integer`);
      }
      if (field.min !== undefined && value < field.min) {
        throw new PluginLoadError(`${where} must be >= ${field.min}`);
      }
      if (field.max !== undefined && value > field.max) {
        throw new PluginLoadError(`${where} must be <= ${field.max}`);
      }
      return value;
    }
    case "string":
      if (typeof value !== "string") throw new PluginLoadError(`${where} must be a string`);
      if (field.enum !== undefined && !field.enum.includes(value)) {
        throw new PluginLoadError(`${where} must be one of: ${field.enum.join(", ")}`);
      }
      return value;
    case "stringArray":
      if (!Array.isArray(value) || !value.every((entry) => typeof entry === "string")) {
        throw new PluginLoadError(`${where} must be an array of strings`);
      }
      return [...value];
  }
}

/**
 * Validate manifest-side options against `schema`, filling defaults.
 * `true` (or absent) means "defaults"; without a schema any options object
 * is refused, so a typo'd option fails the load instead of being ignored.
 */
export function applyCapabilityOptions(
  capability: string,
  schema: CapabilityOptionsSchema | undefined,
  options: unknown,
): Record<string, unknown> {
  const input = options === undefined || options === true ? {} : options;
  if (
    typeof input !== "object" ||
    input === null ||
    Array.isArray(input) ||
    Object.getPrototypeOf(input) !== Object.prototype
  ) {
    throw new PluginLoadError(`capability "${capability}": options must be an object`);
  }
  const entries = Object.entries(input);
  if (schema === undefined) {
    if (entries.length > 0) {
      throw new PluginLoadError(
        `capability "${capability}" takes no options (got "${entries[0][0]}")`,
      );
    }
    return {};
  }
  const out: Record<string, unknown> = {};
  for (const [key, value] of entries) {
    const field = schema[key];
    if (field === undefined) {
      throw new PluginLoadError(`capability "${capability}": unknown option "${key}"`);
    }
    out[key] = checkField(capability, key, field, value);
  }
  for (const [key, field] of Object.entries(schema)) {
    if (!(key in out) && field.default !== undefined) out[key] = field.default;
  }
  return out;
}

// ── registry ───────────────────────────────────────────────────────

/** Names are manifest keys, so they stay identifier-ish and log-safe. */
export const CAPABILITY_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9:_.-]*$/;

const registry = new Map<string, CapabilityDefinition>();

function assertName(name: string): void {
  if (typeof name !== "string" || !CAPABILITY_NAME_PATTERN.test(name)) {
    throw new Error(`capability name must match ${CAPABILITY_NAME_PATTERN}`);
  }
}

/** Register a capability. Throws when the name is taken — no shadowing. */
export function defineCapability(definition: CapabilityDefinition): void {
  assertName(definition.name);
  if (
    definition.install !== undefined &&
    typeof definition.install !== "function"
  ) {
    throw new Error(`capability "${definition.name}" needs an install function`);
  }
  if (registry.has(definition.name)) {
    throw new Error(`capability "${definition.name}" is already defined`);
  }
  registry.set(definition.name, definition);
}

export function hasCapability(name: string): boolean {
  return registry.has(name);
}

export function getCapability(name: string): CapabilityDefinition | undefined {
  return registry.get(name);
}

/** Registered names, sorted — deterministic load order and docs. */
export function listCapabilities(): string[] {
  return [...registry.keys()].sort();
}

/** Remove a registration (operator teardown, tests). `false` when absent. */
export function unregisterCapability(name: string): boolean {
  return registry.delete(name);
}

// ── the `capabilities` map: the generic extension point ────────────

defineCapability({
  name: "capabilities",
  // Shape only: whether a name exists is decided at load time against
  // the registry, so a typo fails the load instead of the parse.
  validate(value, field) {
    if (
      typeof value !== "object" ||
      value === null ||
      Array.isArray(value)
    ) {
      throw new PluginManifestError(`${field} must be an object`);
    }
    const requested = value as Record<string, unknown>;
    const compiled: Record<string, CapabilityRequest> = {};
    for (const [name, entry] of Object.entries(requested)) {
      if (!CAPABILITY_NAME_PATTERN.test(name)) {
        throw new PluginManifestError(
          `${field} has an invalid capability name "${name}"`,
        );
      }
      if (typeof entry === "boolean") {
        compiled[name] = entry;
      } else if (
        typeof entry === "object" &&
        entry !== null &&
        !Array.isArray(entry) &&
        Object.getPrototypeOf(entry) === Object.prototype
      ) {
        compiled[name] = entry as Record<string, unknown>;
      } else {
        throw new PluginManifestError(
          `${field}["${name}"] must be a boolean or an options object`,
        );
      }
    }
    return compiled;
  },
  // One map entry becomes one capability install; unknown names fail the
  // load at registry lookup, exactly like a directly requested name.
  resolve: (request) =>
    Object.entries(request as Record<string, unknown>).map(([capability, options]) => ({
      capability,
      options,
    })),
});
