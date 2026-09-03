/**
 * A capability-based plugin system for `napi-vm`.
 *
 * The VM stays sealed: plugins never see `require`, `process`, `node:fs`,
 * `Bun` or `Deno`. They see `napi:fs` and `napi:path` — small host modules
 * whose every privileged call is checked against the plugin's manifest *and*
 * the host policy before it touches the filesystem.
 *
 * ```ts
 * import { PluginHost } from "napi-vm/plugins";
 *
 * const host = new PluginHost({
 *   policy: { fs: { absoluteRead: false, absoluteWrite: false } },
 * });
 *
 * host.load("./examples/plugins/example-plugin");
 * host.reload("example-plugin");
 * host.unload("example-plugin");
 * ```
 *
 * Layout: `core/` (host engine), `capabilities/` (guest modules),
 * `native/` (npm/`.node` bridging), `fs/` (filesystem + path support).
 */

// ── core: host engine ──────────────────────────────────────────────

export {
  PermissionDeniedError,
  PluginLoadError,
  PluginManifestError,
  ResourceLimitError,
} from "./core/errors";

export {
  booleanPermissionSchema,
  definePermissionBinding,
  definePermissionSchema,
  getPermissionBinding,
  hasPermissionBinding,
  hasPermissionSchema,
  isPermissionGranted,
  listPermissionBindings,
  listPermissionSchemas,
  parseManifest,
  unregisterPermissionBinding,
  unregisterPermissionSchema,
  validateManifest,
  SUPPORTED_API_VERSION,
  type PermissionBinding,
  type PermissionResolved,
  type PermissionSchema,
  type PluginManifest,
} from "./core/manifest";

export {
  compilePermissions,
  type CapabilityGrant,
  type CompiledPermissions,
} from "./core/permissions";

export {
  bootstrapSource,
  describePlugin,
  pluginModuleName,
  uninstallLifecycle,
  LIFECYCLE_GLOBALS,
  PLUGIN_MODULE_PREFIX,
  type PluginContext,
  type PluginShape,
  type UnloadContext,
  type UnloadReason,
} from "./core/lifecycle";

export {
  PluginHost,
  defaultPolicy,
  MANIFEST_FILENAME,
  type LoadedPlugin,
  type PluginHostOptions,
  type PluginHostPolicy,
} from "./core/plugin-host";

// ── fs: filesystem + path + permission-check support ───────────────

export {
  compileFsPolicy,
  defaultFsPolicy,
  FsPermissionChecker,
  type CompiledFsPermissions,
  type CompiledFsPolicy,
  type FsAccessMode,
  type FsPolicyOptions,
  type ResolvedPath,
} from "./fs/checker";

export {
  createNodeFileSystem,
  DEFAULT_MAX_FILE_BYTES,
  type HostFileSystem,
  type NodeFileSystemOptions,
} from "./fs/host-filesystem";

export {
  compilePattern,
  escapesRoot,
  matchRule,
  normalizeSegments,
  toPosix,
  isAbsoluteGuestPath,
  validateEntryPath,
  type PathRule,
  type PathRuleKind,
} from "./fs/path-rules";

// ── capabilities: guest modules ────────────────────────────────────

export {
  applyCapabilityOptions,
  defineCapability,
  getCapability,
  hasCapability,
  listCapabilities,
  unbindCapabilityModule,
  unregisterCapability,
  CAPABILITY_NAME_PATTERN,
  type CapabilityContext,
  type CapabilityDefinition,
  type CapabilityFieldSchema,
  type CapabilityOptionsSchema,
  type CapabilityRequest,
  type CapabilityTeardown,
} from "./capabilities/capability-registry";

export {
  compileFsPermission,
  installFsCapability,
  FS_MODULE_NAME,
  type FsCapabilityOptions,
  type FsPermission,
} from "./capabilities/filesystem-capability";

export {
  installPathCapability,
  PATH_CAPABILITY,
  PATH_MODULE_NAME,
} from "./capabilities/path-capability";

export {
  CRYPTO_CAPABILITY,
  CRYPTO_MODULE_NAME,
  MAX_RANDOM_BYTES,
} from "./capabilities/crypto-capability";

export {
  TIMERS_CAPABILITY,
  TIMERS_MODULE_NAME,
  type TimersCapabilityOptions,
} from "./capabilities/timers-capability";

export {
  checkFetchOrigin,
  compileFetchPermission,
  FETCH_CAPABILITY,
  DEFAULT_MAX_RESPONSE_BYTES,
  FETCH_MODULE_NAME,
  type CompiledFetchPermissions,
  type FetchPermission,
  type FetchPolicy,
  type FetchTransport,
} from "./capabilities/fetch-capability";

export {
  AUDIO_CAPABILITY,
  AUDIO_DEFINITION,
  AUDIO_MODULE_NAME,
  DEFAULT_MAX_AUDIO_BYTES,
  type AudioPlayerLike,
  type AudioPolicyOptions,
} from "./capabilities/audio-capability";

// ── native: npm / `.node` bridging (host-side, operator-gated) ─────

export {
  installNativeModule,
  uninstallNativeModule,
  type InstalledNativeModule,
  type NativeMethodPolicy,
  type NativeModuleDefinition,
  type NativeModuleHost,
} from "./native/native-loader";

export {
  assertTrustedSpec,
  ensureModulesDir,
  extractTarball,
  installTrustedPackage,
  nativePackageCapability,
  packageTarballUrl,
  verifyIntegrity,
  DEFAULT_MODULES_DIR,
  DEFAULT_REGISTRY,
  MAX_TARBALL_BYTES,
  MAX_TARBALL_FILES,
  type LoadedTrustedPackage,
  type NativePackageCapabilityOptions,
  type TrustedModulesPolicy,
  type TrustedPackageSpec,
} from "./native/trusted-modules";
