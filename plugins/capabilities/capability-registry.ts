/**
 * Dynamic capability registry: names are data, not syntax — and every
 * capability shares one lifecycle.
 *
 * A capability is a small subplugin: it declares a `name`, an option
 * `schema` (validated with defaults applied), and an `install` that wires
 * guest modules and returns its own teardown. There is no
 * `uninstallXCapability` per capability; the host runs the returned
 * teardowns on unload, reload and failed hooks.
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

import { PluginLoadError } from "../core/errors";
import type { CompiledPermissions, FsPermissionChecker } from "../core/permissions";
import type { PluginManifest } from "../core/manifest";

/** What an installed capability may use. */
export interface CapabilityContext {
  vm: Vm;
  manifest: PluginManifest;
  permissions: CompiledPermissions;
  /**
   * The loading plugin's path checker. Optional because most capabilities
   * never touch a path; definitions that expose a path sink fail closed
   * without one (see `native-loader.ts`).
   */
  checker?: FsPermissionChecker;
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

export interface CapabilityDefinition {
  /** Registry name, e.g. `"audio"`. Validated, not free-form. */
  readonly name: string;
  /**
   * Manifest-side options, validated with defaults applied before `install`
   * runs. Absent means "no options": `true` installs with `{}`, and any
   * options object is refused.
   */
  readonly schema?: CapabilityOptionsSchema;
  install(ctx: CapabilityContext): CapabilityTeardown;
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
  if (typeof definition.install !== "function") {
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
