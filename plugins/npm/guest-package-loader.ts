import type { Vm } from "../../index";
import type { HostPlatform } from "../platform";
import { IdentityCompiler, SwcCompiler } from "./compiler";
import { scanModuleSource, rewriteModuleSpecifiers } from "./module-graph";
import { GuestPackageResolver, canonicalModuleId, type ResolvedPackageModule } from "./resolver";
import { NpmCompatibilityError, type CompilerMode, type GuestCompiler, type GuestSyntax } from "./types";

export interface GuestPackageLoaderOptions {
  platform: HostPlatform;
  /** Directory to search when resolving the first bare package specifier. */
  rootDir?: string;
  /** Defaults to `none` so VM parser gaps remain visible. */
  compilerMode?: CompilerMode;
  /** Compiler override, useful for embedding a controlled compiler backend. */
  compiler?: GuestCompiler;
}

interface PendingModule {
  resolved: ResolvedPackageModule;
  id: string;
}

interface PreparedModule {
  id: string;
  source: string;
}

function syntaxFor(filename: string): GuestSyntax {
  switch (filename.slice(filename.lastIndexOf(".") + 1).toLowerCase()) {
    case "jsx":
      return "jsx";
    case "ts":
      return "ts";
    case "tsx":
      return "tsx";
    default:
      return "js";
  }
}

function diagnosticText(diagnostics: ReturnType<Vm["validateModule"]>["diagnostics"]): string {
  return diagnostics
    .map(({ line, column, kind, message }) => `${kind} at ${line}:${column}: ${message}`)
    .join("\n");
}

/**
 * Read and register pure JavaScript npm packages as guest ESM modules.
 * Package code is only read as text; this loader never calls host `require`
 * or `import` on package entry points.
 */
export class GuestPackageLoader {
  private readonly resolver: GuestPackageResolver;
  private readonly rootDir: string;
  private readonly compilerMode: CompilerMode;
  private readonly compiler: GuestCompiler;
  private readonly platform: HostPlatform;
  private readonly loaded = new Map<string, string>();
  private readonly registeredModules = new Set<string>();

  constructor(private readonly vm: Vm, options: GuestPackageLoaderOptions) {
    this.platform = options.platform;
    const path = options.platform.path;
    this.rootDir = path.resolve(options.rootDir ?? path.cwd());
    this.compilerMode = options.compilerMode ?? "none";
    this.compiler = options.compiler ??
      (this.compilerMode === "none" ? new IdentityCompiler() : new SwcCompiler());
    this.resolver = new GuestPackageResolver({ fs: options.platform.fs, path, rootDir: this.rootDir });
  }

  /** Resolve, compile if configured, validate, and register a package graph. */
  async loadPackage(specifier: string): Promise<string> {
    const cached = this.loaded.get(specifier);
    if (
      cached &&
      this.registeredModules.has(cached) &&
      this.registeredModules.has(specifier) &&
      [...this.registeredModules].every((name) => this.vm.hasModule(name))
    ) {
      return cached;
    }
    if (cached) this.loaded.delete(specifier);

    const entry = this.resolver.resolvePackage(specifier);
    const entryId = canonicalModuleId(entry.package, entry.filename, this.resolverPath());
    const pending: PendingModule[] = [{ resolved: entry, id: entryId }];
    const scheduled = new Set([entryId]);
    const prepared = new Map<string, PreparedModule>();

    while (pending.length > 0) {
      const item = pending.shift()!;
      if (prepared.has(item.id)) continue;
      const rawSource = this.readSource(item.resolved.filename);
      const code = await this.prepareSource(item.resolved.filename, rawSource);
      const scanned = scanModuleSource(code);
      if (scanned.hasCommonJs) {
        throw new NpmCompatibilityError(
          "MODULE_RESOLUTION",
          `CommonJS source is not supported in guest packages: ${item.resolved.filename}`,
        );
      }
      if (scanned.hasNonLiteralDynamicImport) {
        throw new NpmCompatibilityError(
          "MODULE_RESOLUTION",
          `Cannot build a closed guest module graph for non-literal dynamic import in ${item.resolved.filename}`,
        );
      }
      const targets: string[] = [];

      for (const imported of scanned.imports) {
        const resolved = imported.specifier.startsWith(".")
          ? this.resolver.resolveRelative(imported.specifier, item.resolved)
          : this.resolver.resolvePackage(imported.specifier, item.resolved.filename);
        const targetId = canonicalModuleId(resolved.package, resolved.filename, this.resolverPath());
        targets.push(targetId);
        if (!scheduled.has(targetId)) {
          scheduled.add(targetId);
          pending.push({ resolved, id: targetId });
        }
      }

      const linkedSource = rewriteModuleSpecifiers(code, scanned.imports, targets);
      const finalValidation = this.vm.validateModule(linkedSource);
      if (!finalValidation.valid) {
        throw new NpmCompatibilityError(
          this.compilerMode === "none" ? "UNSUPPORTED_SYNTAX" : "SWC_TRANSFORM",
          `napi-vm rejected ${item.resolved.filename} after package linking:\n${diagnosticText(finalValidation.diagnostics)}`,
        );
      }
      prepared.set(item.id, { id: item.id, source: linkedSource });
    }

    // The entry was processed first; inspect the transformed graph's entry to
    // preserve its default export when a bare-name alias is installed.
    const entrySource = prepared.get(entryId)?.source ?? "";
    const entryScan = scanModuleSource(entrySource);
    const aliasSource = entryScan.hasDefaultExport
      ? `export * from ${JSON.stringify(entryId)};\nexport { default } from ${JSON.stringify(entryId)};`
      : `export * from ${JSON.stringify(entryId)};`;
    const aliasValidation = this.vm.validateModule(aliasSource);
    if (!aliasValidation.valid) {
      throw new NpmCompatibilityError(
        "UNSUPPORTED_SYNTAX",
        `napi-vm rejected package alias ${specifier}:\n${diagnosticText(aliasValidation.diagnostics)}`,
      );
    }

    const registered: string[] = [];
    const knownModules = new Set(
      [...this.registeredModules].filter((name) => this.vm.hasModule(name)),
    );
    try {
      for (const module of prepared.values()) {
        if (this.vm.hasModule(module.id) && !knownModules.has(module.id)) {
          throw new NpmCompatibilityError(
            "MODULE_RESOLUTION",
            `Guest module ID is already registered: ${module.id}`,
          );
        }
      }
      if (specifier !== entryId && this.vm.hasModule(specifier) && !knownModules.has(specifier)) {
        throw new NpmCompatibilityError(
          "MODULE_RESOLUTION",
          `Guest package alias is already registered: ${specifier}`,
        );
      }
      for (const module of prepared.values()) {
        if (knownModules.has(module.id)) continue;
        this.vm.defineModule(module.id, module.source);
        registered.push(module.id);
      }
      if (specifier !== entryId && !knownModules.has(specifier)) {
        this.vm.defineModule(specifier, aliasSource);
        registered.push(specifier);
      }
    } catch (cause) {
      for (const name of registered) this.vm.removeModule(name);
      throw new NpmCompatibilityError(
        "MODULE_RESOLUTION",
        `Could not register package ${specifier}: ${cause instanceof Error ? cause.message : String(cause)}`,
        { cause },
      );
    }

    this.loaded.set(specifier, entryId);
    for (const name of registered) this.registeredModules.add(name);
    return entryId;
  }

  private readSource(filename: string): string {
    const extension = this.resolverPath().extname(filename).toLowerCase();
    if (![".js", ".mjs", ".jsx", ".ts", ".tsx"].includes(extension)) {
      throw new NpmCompatibilityError(
        "NATIVE_DEPENDENCY",
        `Guest packages must provide JavaScript ESM source; cannot load ${filename}`,
      );
    }
    return this.platformFs().readText(filename);
  }

  private async prepareSource(filename: string, source: string): Promise<string> {
    if (this.compilerMode === "none") {
      const validation = this.vm.validateModule(source);
      if (!validation.valid) {
        throw new NpmCompatibilityError(
          "UNSUPPORTED_SYNTAX",
          `napi-vm rejected ${filename}:\n${diagnosticText(validation.diagnostics)}`,
        );
      }
      return source;
    }

    if (this.compilerMode === "auto") {
      const validation = this.vm.validateModule(source);
      if (validation.valid) return source;
    }

    const compiled = await this.compiler.compile({ filename, source, syntax: syntaxFor(filename) });
    const validation = this.vm.validateModule(compiled.code);
    if (!validation.valid) {
      throw new NpmCompatibilityError(
        "SWC_TRANSFORM",
        `napi-vm rejected SWC output for ${filename}:\n${diagnosticText(validation.diagnostics)}`,
      );
    }
    return compiled.code;
  }

  private platformFs() {
    return this.platform.fs;
  }

  private resolverPath() {
    return this.platform.path;
  }
}
