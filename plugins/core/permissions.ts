/**
 * Generic permission compilation: manifest requests keyed by permission key.
 *
 * This module knows no capability and no filesystem. Each permission key is
 * compiled through the capability registered for it (see
 * `capabilities/capability-registry`); the host intersects the result with
 * the per-capability policy grant at install.
 */

import { PluginManifestError } from "./errors";
import { getCapability } from "../capabilities/capability-registry";
import type { PluginManifest } from "./manifest";

/**
 * Compiled manifest requests, keyed by permission key. Each value is the
 * capability's `compile` output (or the request as-is). Installers read
 * their own key with a cast — sound because the capability ran at load.
 */
export type CompiledPermissions = Record<string, unknown>;

/** One capability grant: `true` for defaults, or granted options. */
export type CapabilityGrant = boolean | Record<string, unknown>;

/**
 * Compile every permission a manifest requests, through each key's
 * capability. Keys without a registration fail closed — unreachable for
 * parsed manifests (unknown keys already failed the parse), but possible for
 * hand-built ones.
 */
export function compilePermissions(manifest: PluginManifest): CompiledPermissions {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(manifest.permissions ?? {})) {
    const definition = getCapability(key);
    if (!definition) {
      throw new PluginManifestError(`no permission binding for "${key}"`);
    }
    out[key] = definition.compile ? definition.compile(value) : value;
  }
  return out;
}
