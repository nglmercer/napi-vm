/**
 * Generic permission compilation: manifest requests keyed by permission key.
 *
 * This module knows no capability and no filesystem. Each permission key is
 * compiled through its own `PermissionBinding` (see `./manifest`); the host
 * intersects the result with the per-capability policy grant at install.
 */

import { PluginManifestError } from "./errors";
import { getPermissionBinding } from "./manifest";
import type { PluginManifest } from "./manifest";

/**
 * Compiled manifest requests, keyed by permission key. Each value is the
 * binding's `compileRequest` output (or the request as-is). Installers read
 * their own key with a cast — sound because the binding ran at load.
 */
export type CompiledPermissions = Record<string, unknown>;

/** One capability grant: `true` for defaults, or granted options. */
export type CapabilityGrant = boolean | Record<string, unknown>;

/**
 * Compile every permission a manifest requests, through each key's binding.
 * Keys without a binding fail closed — unreachable for parsed manifests
 * (unknown keys already failed the parse), but possible for hand-built ones.
 */
export function compilePermissions(manifest: PluginManifest): CompiledPermissions {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(manifest.permissions ?? {})) {
    const binding = getPermissionBinding(key);
    if (!binding) {
      throw new PluginManifestError(`no permission binding for "${key}"`);
    }
    out[key] = binding.compileRequest ? binding.compileRequest(value) : value;
  }
  return out;
}
