/**
 * `plugin.json` types and validation.
 *
 * A manifest is a *request*, never an authorization: it is validated here and
 * intersected with the host policy later (see `permissions.ts`).
 */

import { CAPABILITY_NAME_PATTERN } from "../capabilities/capability-registry";
import { PluginManifestError } from "./errors";
import { compileFetchPermission } from "../capabilities/fetch-capability";
import {
  escapesRoot,
  isAbsoluteGuestPath,
  normalizeSegments,
  toPosix,
} from "../fs/path-rules";

/** The only manifest API version this host understands. */
export const SUPPORTED_API_VERSION = 1;

export type FsPermission = boolean | "*" | string | string[];

/**
 * One dynamic capability request: `true` for defaults, or an options object
 * the capability's installer validates. Names resolve against the host's
 * capability registry at load time — an unknown name fails the load.
 */
export type CapabilityRequest = boolean | Record<string, unknown>;

export interface PluginManifest {
  name: string;
  version: string;
  apiVersion: number;
  entry: string;
  permissions?: {
    fs?: {
      read?: FsPermission;
      write?: FsPermission;
    };
    path?: boolean;
    crypto?: boolean;
    timers?: boolean;
    /**
     * Dynamic capabilities: `{ "<name>": true }` or `{ "<name>": {...} }`.
     * Request ∩ host grant = installed. Unknown names fail the load.
     */
    capabilities?: Record<string, CapabilityRequest>;
    /**
     * Origins the plugin asks to reach. `true`/`"*"` requests any, which the
     * host policy must still permit.
     */
    fetch?: boolean | string | string[];
  };
}

const NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

function requireString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new PluginManifestError(`${field} must be a non-empty string`);
  }
  return value;
}

function validateFsPermission(value: unknown, field: string): FsPermission | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "boolean" || typeof value === "string") return value;
  if (Array.isArray(value)) {
    for (const entry of value) {
      if (typeof entry !== "string") {
        throw new PluginManifestError(`${field} array entries must be strings`);
      }
    }
    return value as string[];
  }
  throw new PluginManifestError(`${field} must be boolean, string, or string[]`);
}

/**
 * Validate the entry path: it must be a relative POSIX path that stays inside
 * the plugin directory.
 */
function validateEntry(value: unknown): string {
  const raw = requireString(value, "entry");
  const posix = toPosix(raw);
  if (isAbsoluteGuestPath(posix)) {
    throw new PluginManifestError("entry must be a path inside the plugin directory");
  }
  const normalized = normalizeSegments(posix, false);
  if (normalized === "" || escapesRoot(normalized)) {
    throw new PluginManifestError("entry must be a path inside the plugin directory");
  }
  return normalized;
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

  const entry = validateEntry(source.entry);

  const manifest: PluginManifest = { name, version, apiVersion, entry };

  const permissions = source.permissions;
  if (permissions !== undefined) {
    if (typeof permissions !== "object" || permissions === null || Array.isArray(permissions)) {
      throw new PluginManifestError("permissions must be an object");
    }
    const perms = permissions as Record<string, unknown>;
    manifest.permissions = {};

    if (perms.fs !== undefined) {
      if (typeof perms.fs !== "object" || perms.fs === null || Array.isArray(perms.fs)) {
        throw new PluginManifestError("permissions.fs must be an object");
      }
      const fs = perms.fs as Record<string, unknown>;
      manifest.permissions.fs = {};
      const read = validateFsPermission(fs.read, "permissions.fs.read");
      const write = validateFsPermission(fs.write, "permissions.fs.write");
      if (read !== undefined) manifest.permissions.fs.read = read;
      if (write !== undefined) manifest.permissions.fs.write = write;
    }

    if (perms.path !== undefined) {
      if (typeof perms.path !== "boolean") {
        throw new PluginManifestError("permissions.path must be a boolean");
      }
      manifest.permissions.path = perms.path;
    }

    for (const flag of ["crypto", "timers"] as const) {
      if (perms[flag] !== undefined) {
        if (typeof perms[flag] !== "boolean") {
          throw new PluginManifestError(`permissions.${flag} must be a boolean`);
        }
        manifest.permissions[flag] = perms[flag] as boolean;
      }
    }

    if (perms.capabilities !== undefined) {
      // Shape only: whether a name exists is decided at load time against
      // the host registry, so a typo fails the load instead of the parse.
      if (
        typeof perms.capabilities !== "object" ||
        perms.capabilities === null ||
        Array.isArray(perms.capabilities)
      ) {
        throw new PluginManifestError("permissions.capabilities must be an object");
      }
      const requested = perms.capabilities as Record<string, unknown>;
      const compiled: Record<string, CapabilityRequest> = {};
      for (const [name, value] of Object.entries(requested)) {
        if (!CAPABILITY_NAME_PATTERN.test(name)) {
          throw new PluginManifestError(
            `permissions.capabilities has an invalid capability name "${name}"`,
          );
        }
        if (typeof value === "boolean") {
          compiled[name] = value;
        } else if (
          typeof value === "object" &&
          value !== null &&
          !Array.isArray(value) &&
          Object.getPrototypeOf(value) === Object.prototype
        ) {
          compiled[name] = value as Record<string, unknown>;
        } else {
          throw new PluginManifestError(
            `permissions.capabilities["${name}"] must be a boolean or an options object`,
          );
        }
      }
      manifest.permissions.capabilities = compiled;
    }

    if (perms.fetch !== undefined) {
      // Validated eagerly, so a malformed origin fails at load time rather
      // than on the first request — the same rule the path patterns follow.
      compileFetchPermission(perms.fetch);
      manifest.permissions.fetch = perms.fetch as boolean | string | string[];
    }
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
