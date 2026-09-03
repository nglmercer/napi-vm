/**
 * `plugin.json` types and validation.
 *
 * A manifest is a *request*, never an authorization: it is validated here and
 * intersected with the host policy later (see `permissions.ts`).
 *
 * This module names no permission key. Each key is validated by the
 * `validate` hook of the capability registered for it — the filesystem shape
 * lives with the filesystem capability, the fetch shape with the fetch
 * capability, and so on. New capabilities add keys without touching this
 * file, and a key nobody registered fails the load instead of being silently
 * ignored.
 */

import { getCapability } from "../capabilities/capability-registry";
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
   * Per-key validated by the registered capability for that key. Values are
   * the validated forms; consumers read known keys with a cast (sound because
   * the capability's `validate` ran at parse time).
   */
  permissions?: Record<string, unknown>;
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
      const definition = getCapability(key);
      if (!definition) {
        throw new PluginManifestError(`unknown permission "${key}"`);
      }
      compiled[key] = definition.validate
        ? definition.validate(value, `permissions.${key}`)
        : value;
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
