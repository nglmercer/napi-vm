/**
 * Node/Bun adapter for the plugin system (`"napi-vm/plugins/node"`).
 *
 * Everything here may import `node:*` — that is the point. Importing this
 * module (directly or transitively) ties the bundle to Node/Bun, so portable
 * hosts (Tauri renderers, workers, single-file desktop builds) import only
 * `"napi-vm/plugins"` and assemble `portablePlatform(theirFileSystem)`.
 *
 * ```ts
 * import { PluginHost } from "napi-vm/plugins";
 * import { nodePlatform } from "napi-vm/plugins/node";
 *
 * const host = new PluginHost({ policy, platform: nodePlatform() });
 * ```
 */

// ── platform: filesystem, paths, crypto, native loading ────────────

export {
  nodeCrypto,
  nodePlatform,
  type NodePlatformOptions,
} from "./node/node-platform";

export {
  createNodeFileSystem,
  type NodeFileSystemOptions,
} from "./node/node-filesystem";

export { createMiniaudioPlayer } from "./node/miniaudio";

// ── native: npm / `.node` bridging (host-side, operator-gated) ─────

export {
  installTrustedPackage,
  nativePackageCapability,
  assertTrustedSpec,
  ensureModulesDir,
  extractTarball,
  verifyIntegrity,
  packageTarballUrl,
  DEFAULT_MODULES_DIR,
  DEFAULT_REGISTRY,
  MAX_TARBALL_BYTES,
  MAX_TARBALL_FILES,
  type LoadedTrustedPackage,
  type NativePackageCapabilityOptions,
  type TrustedModulesPolicy,
  type TrustedPackageSpec,
} from "./native/trusted-modules";
