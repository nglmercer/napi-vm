/**
 * A capability-based plugin system for `napi-vm`.
 *
 * The VM stays sealed: plugins do not receive `process`, `Bun` or `Deno`.
 * Standard module facades such as `node:fs` are installed by the host and
 * check each privileged call against the plugin manifest and host policy.
 *
 * This barrel is portable: nothing in its import graph touches `node:*` or
 * any npm package at runtime (type-only imports are erased). Hosts running
 * on Node/Bun add `"napi-vm/plugins/node"` for the ready-made platform
 * (`nodePlatform()`) and the trusted-package installer:
 *
 * ```ts
 * import { PluginHost } from "napi-vm/plugins";
 * import { nodePlatform } from "napi-vm/plugins/node";
 *
 * const host = new PluginHost({ policy: { capabilities: {} }, platform: nodePlatform() });
 *
 * host.load("./examples/plugins/example-plugin");
 * host.reload("example-plugin");
 * host.unload("example-plugin");
 * ```
 *
 * Layout: `platform.ts` (portable outside-world interfaces), `core/` (host
 * engine), `capabilities/` (guest modules), `fs/` (path support). Node-only
 * code lives in `node/` and `native/` and is exported from `node.ts`, never
 * here.
 */

// ── platform: the portable outside world ───────────────────────────

export {
  DEFAULT_MAX_FILE_BYTES,
  missingFileSystem,
  portableCrypto,
  portablePlatform,
  posixPath,
  type HostCrypto,
  type HostFileSystem,
  type HostPath,
  type HostPlatform,
} from "./platform";

// ── core: host engine ──────────────────────────────────────────────

export {
  PermissionDeniedError,
  PluginLoadError,
  PluginManifestError,
  ResourceLimitError,
} from "./core/errors";

export {
  parseManifest,
  SUPPORTED_API_VERSION,
  validateManifest,
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
  installNativeModule,
  uninstallNativeModule,
  type InstalledNativeModule,
  type NativeMethodPolicy,
  type NativeModuleDefinition,
  type NativeModuleHost,
} from "./core/native-bridge";

export {
  PluginHost,
  defaultPolicy,
  MANIFEST_FILENAME,
  type LoadedPlugin,
  type PluginHostOptions,
  type PluginHostPolicy,
} from "./core/plugin-host";

// ── fs: path support ───────────────────────────────────────────────

export {
  FsPermissionChecker,
  type CompiledFsPermissions,
  type FsAccessMode,
  type ResolvedPath,
} from "./fs/checker";

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
  booleanPermissionValue,
  defineCapability,
  getCapability,
  hasCapability,
  isPermissionGranted,
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
  type PermissionResolved,
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
  type CompiledFetchPermissions,
  type FetchPermission,
  type FetchPolicy,
  type FetchTransport,
} from "./capabilities/fetch-capability";

// ── npm: pure guest package source ─────────────────────────────────

export {
  GuestPackageLoader,
  GuestPackageResolver,
  IdentityCompiler,
  SwcCompiler,
  NpmCompatibilityError,
  canonicalModuleId,
  parsePackageJson,
  parsePackageSpecifier,
  resolveExportTarget,
  rewriteModuleSpecifiers,
  scanModuleSource,
  type CompilerMode,
  type GuestCompiler,
  type GuestCompilerInput,
  type GuestCompilerOutput,
  type GuestPackage,
  type GuestPackageJson,
  type GuestPackageLoaderOptions,
  type GuestSyntax,
  type NpmCompatibilityCategory,
  type ResolvedPackageModule,
  type ScannedModuleSource,
  type StaticImport,
  type SwcBackend,
  type SwcTransformOptions,
  type SwcTransformResult,
} from "./npm";
