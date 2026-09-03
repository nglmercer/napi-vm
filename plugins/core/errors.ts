/**
 * Error types shared by the plugin host.
 *
 * `PermissionDeniedError` and `ResourceLimitError` are the ones that cross
 * into the VM. The
 * interpreter carries `name` across, so the guest sees
 * `e.name === "PermissionDenied"` with the bare detail as `e.message`, and a
 * host-side `callFunction` sees them rejoined as `"PermissionDenied: …"`.
 *
 * Messages must never contain host filesystem paths — a guest may only see the
 * path string it passed in itself.
 */

/** A path denied by host policy or by the plugin's manifest permissions. */
export class PermissionDeniedError extends Error {
  override readonly name = "PermissionDenied";

  constructor(detail: string) {
    super(detail);
  }
}

/** A malformed or unsupported `plugin.json`. */
export class PluginManifestError extends Error {
  override readonly name = "PluginManifestError";

  constructor(detail: string) {
    super(`PluginManifestError: ${detail}`);
  }
}

/** A failure while loading, reloading or unloading a plugin. */
export class PluginLoadError extends Error {
  override readonly name = "PluginLoadError";

  constructor(detail: string, options?: { cause?: unknown }) {
    super(`PluginLoadError: ${detail}`, options);
  }
}

/**
 * A permitted operation refused for exceeding a host resource limit.
 *
 * Distinct from `PermissionDeniedError` on purpose: the path *was* authorized,
 * so reporting it as a permission failure would send a plugin author looking
 * at their manifest instead of at the size of the file.
 *
 * Like `PermissionDeniedError`, messages must never contain host paths.
 */
export class ResourceLimitError extends Error {
  override readonly name = "ResourceLimit";

  constructor(detail: string) {
    super(detail);
  }
}
