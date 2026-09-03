/**
 * `plugin.json` types and validation.
 *
 * A manifest is a *request*, never an authorization: it is validated here and
 * intersected with the host policy later (see `permissions.ts`).
 *
 * This module is agnostic to every capability: it names no permission key.
 * Each key is validated by a `PermissionSchema` registered for it — the
 * filesystem shape lives with the filesystem capability, the fetch shape
 * with the fetch capability, and so on. New capabilities add keys without
 * touching this file, and a key nobody registered fails the load instead of
 * being silently ignored.
 *
 * Schemas register as a side effect of importing their module, so consumers
 * import the `plugins` barrel (never this file alone).
 */

import { PluginManifestError } from "./errors";

/** The only manifest API version this host understands. */
export const SUPPORTED_API_VERSION = 1;

export interface PluginManifest {
  name: string;
  version: string;
  apiVersion: number;
  /** Raw entry string; the host normalizes it (`validateEntryPath`). */
  entry: string;
  /**
   * Per-key validated by the registered `PermissionSchema` for that key.
   * Values are the schema-normalized forms; consumers read known keys with
   * a cast (sound because the schema ran at parse time).
   */
  permissions?: Record<string, unknown>;
}

/**
 * Validates and normalizes one `permissions.<key>` value. Throws
 * `PluginManifestError` on anything the key does not accept.
 */
export interface PermissionSchema {
  validate(value: unknown, field: string): unknown;
}

const permissionSchemas = new Map<string, PermissionSchema>();

const PERMISSION_KEY_PATTERN = /^[A-Za-z][A-Za-z0-9_-]*$/;

/** Register the validator for one `permissions.<key>`. Throws when taken. */
export function definePermissionSchema(key: string, schema: PermissionSchema): void {
  if (!PERMISSION_KEY_PATTERN.test(key)) {
    throw new Error(`permission key must match ${PERMISSION_KEY_PATTERN}`);
  }
  if (typeof schema?.validate !== "function") {
    throw new Error(`permission "${key}" needs a validate function`);
  }
  if (permissionSchemas.has(key)) {
    throw new Error(`permission "${key}" is already defined`);
  }
  permissionSchemas.set(key, schema);
}

export function hasPermissionSchema(key: string): boolean {
  return permissionSchemas.has(key);
}

/** Registered permission keys, sorted. */
export function listPermissionSchemas(): string[] {
  return [...permissionSchemas.keys()].sort();
}

/** Remove a registration (operator teardown, tests). `false` when absent. */
export function unregisterPermissionSchema(key: string): boolean {
  return permissionSchemas.delete(key);
}

/** Shared schema for boolean flags (`path`, `crypto`, `timers`, ...). */
export function booleanPermissionSchema(): PermissionSchema {
  return {
    validate(value, field) {
      if (typeof value !== "boolean") {
        throw new PluginManifestError(`${field} must be a boolean`);
      }
      return value;
    },
  };
}

/** One expanded install: which capability, with which options. */
export interface PermissionResolved {
  capability: string;
  options: unknown;
}

/**
 * The policy/install half of a permission key. Schemas validate the manifest
 * side; bindings decide what installs and with what compiled form:
 *
 * - `resolve` expands a manifest request into capability installs. Default:
 *   `[{ capability: key, options: request }]`. The `capabilities` map
 *   expands each entry; infrastructure keys like `fs` resolve to `[]`
 *   (no guest module — their compiled form feeds shared plumbing instead).
 * - `compileRequest` builds the installer-facing form stored in the compiled
 *   map. Default: the request as-is.
 * - `allows` is the request ∩ grant decision. Default: granted unless the
 *   grant is absent, null, or `false`.
 */
export interface PermissionBinding {
  resolve?(request: unknown): PermissionResolved[];
  compileRequest?(request: unknown): unknown;
  allows?(request: unknown, grant: unknown): boolean;
}

/** Default install gate: denied when the grant is absent, null, or `false`. */
export function isPermissionGranted(grant: unknown): boolean {
  return grant !== undefined && grant !== null && grant !== false;
}

const permissionBindings = new Map<string, PermissionBinding>();

/**
 * Register the policy/install behavior for one permission key. Throws when
 * taken. Mirrors `definePermissionSchema(key, schema)` — same key space,
 * one registry per side.
 */
export function definePermissionBinding(key: string, binding: PermissionBinding): void {
  if (!PERMISSION_KEY_PATTERN.test(key)) {
    throw new Error(`permission key must match ${PERMISSION_KEY_PATTERN}`);
  }
  if (
    (binding.resolve !== undefined && typeof binding.resolve !== "function") ||
    (binding.compileRequest !== undefined && typeof binding.compileRequest !== "function") ||
    (binding.allows !== undefined && typeof binding.allows !== "function")
  ) {
    throw new Error(`permission binding "${key}" has an invalid hook`);
  }
  if (permissionBindings.has(key)) {
    throw new Error(`permission binding "${key}" is already defined`);
  }
  permissionBindings.set(key, binding);
}

export function hasPermissionBinding(key: string): boolean {
  return permissionBindings.has(key);
}

export function getPermissionBinding(key: string): PermissionBinding | undefined {
  return permissionBindings.get(key);
}

/** Registered binding keys, sorted. */
export function listPermissionBindings(): string[] {
  return [...permissionBindings.keys()].sort();
}

/** Remove a registration (operator teardown, tests). `false` when absent. */
export function unregisterPermissionBinding(key: string): boolean {
  return permissionBindings.delete(key);
}

const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

function requireString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new PluginManifestError(`${field} must be a non-empty string`);
  }
  return value;
}

/** Validate a parsed `plugin.json` value, returning a normalized manifest. */
export function validateManifest(raw: unknown): PluginManifest {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw new PluginManifestError("manifest must be a JSON object");
  }
  const source = raw as Record<string, unknown>;

  const name = requireString(source.name, "name");
  if (!NAME_PATTERN.test(name)) {
    throw new PluginManifestError(
      "name must match /^[A-Za-z0-9][A-Za-z0-9._-]*$/",
    );
  }

  const version = requireString(source.version, "version");

  const apiVersion = source.apiVersion;
  if (typeof apiVersion !== "number" || !Number.isInteger(apiVersion)) {
    throw new PluginManifestError("apiVersion must be an integer");
  }
  if (apiVersion !== SUPPORTED_API_VERSION) {
    throw new PluginManifestError(
      `apiVersion ${apiVersion} is not supported (expected ${SUPPORTED_API_VERSION})`,
    );
  }

  const entry = requireString(source.entry, "entry");

  const manifest: PluginManifest = { name, version, apiVersion, entry };

  const permissions = source.permissions;
  if (permissions !== undefined) {
    if (typeof permissions !== "object" || permissions === null || Array.isArray(permissions)) {
      throw new PluginManifestError("permissions must be an object");
    }
    const compiled: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(permissions)) {
      const schema = permissionSchemas.get(key);
      if (!schema) {
        throw new PluginManifestError(`unknown permission "${key}"`);
      }
      compiled[key] = schema.validate(value, `permissions.${key}`);
    }
    manifest.permissions = compiled;
  }

  return manifest;
}

/** Parse and validate raw `plugin.json` text. */
export function parseManifest(text: string): PluginManifest {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    throw new PluginManifestError(
      `plugin.json is not valid JSON: ${(error as Error).message}`,
    );
  }
  return validateManifest(parsed);
}
