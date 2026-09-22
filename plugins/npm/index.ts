/** Guest-side npm package resolution and optional source compatibility. */

export { GuestPackageLoader, type GuestPackageLoaderOptions } from "./guest-package-loader";
export { GuestPackageResolver, canonicalModuleId, parsePackageSpecifier, type GuestPackage, type ResolvedPackageModule } from "./resolver";
export { parsePackageJson, resolveExportTarget, type GuestPackageJson } from "./package-json";
export { scanModuleSource, rewriteModuleSpecifiers, type ScannedModuleSource, type StaticImport } from "./module-graph";
export {
  IdentityCompiler,
  SwcCompiler,
  type SwcBackend,
  type SwcTransformOptions,
  type SwcTransformResult,
} from "./compiler";
export {
  NpmCompatibilityError,
  type CompilerMode,
  type GuestCompiler,
  type GuestCompilerInput,
  type GuestCompilerOutput,
  type GuestSyntax,
  type NpmCompatibilityCategory,
} from "./types";
