import type { HostFileSystem, HostPath } from "../platform";
import { parsePackageJson, resolveExportTarget, type GuestPackageJson } from "./package-json";
import { NpmCompatibilityError } from "./types";

export interface GuestPackage {
  name: string;
  version: string;
  root: string;
  packageJsonPath: string;
  packageJson: GuestPackageJson;
}

export interface ResolvedPackageModule {
  package: GuestPackage;
  filename: string;
}

export interface GuestPackageResolverOptions {
  fs: HostFileSystem;
  path: HostPath;
  rootDir: string;
}

export function canonicalModuleId(pkg: GuestPackage, filename: string, path: HostPath): string {
  const relative = path.relative(pkg.root, filename).replaceAll("\\", "/");
  if (relative === ".." || relative.startsWith("../") || path.isAbsolute(relative)) {
    throw new NpmCompatibilityError("MODULE_RESOLUTION", `${filename} escapes package ${pkg.name}`);
  }
  return `/npm/${pkg.name}@${pkg.version}/${relative}`;
}

export function parsePackageSpecifier(specifier: string): { name: string; subpath: string } {
  if (specifier.startsWith("node:")) {
    throw new NpmCompatibilityError(
      "NATIVE_DEPENDENCY",
      `Node built-in ${specifier} is unavailable to guest packages`,
    );
  }
  if (specifier.startsWith(".") || specifier.startsWith("/") || specifier.startsWith("#")) {
    throw new NpmCompatibilityError("MODULE_RESOLUTION", `Expected a bare npm package specifier: ${specifier}`);
  }
  const parts = specifier.split("/");
  const name = specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0];
  if (!name || (name.startsWith("@") && parts.length < 2)) {
    throw new NpmCompatibilityError("MODULE_RESOLUTION", `Invalid npm package specifier: ${specifier}`);
  }
  return { name, subpath: parts.length > (name.startsWith("@") ? 2 : 1) ? `./${parts.slice(name.startsWith("@") ? 2 : 1).join("/")}` : "." };
}

function readPackage(
  fs: HostFileSystem,
  path: HostPath,
  packageRoot: string,
  fallbackName: string,
): GuestPackage {
  const packageJsonPath = fs.exists(packageRoot) ? path.join(packageRoot, "package.json") : "";
  if (!packageJsonPath || !fs.exists(packageJsonPath)) {
    throw new NpmCompatibilityError("MODULE_RESOLUTION", `No package.json found in ${packageRoot}`);
  }
  const packageJson = parsePackageJson(fs.readText(packageJsonPath), packageJsonPath);
  return {
    name: typeof packageJson.name === "string" ? packageJson.name : fallbackName,
    version: typeof packageJson.version === "string" ? packageJson.version : "0.0.0",
    root: packageRoot,
    packageJsonPath,
    packageJson,
  };
}

export class GuestPackageResolver {
  constructor(private readonly options: GuestPackageResolverOptions) {}

  resolvePackage(specifier: string, importerPath?: string): ResolvedPackageModule {
    const { name, subpath } = parsePackageSpecifier(specifier);
    const importerPackage = importerPath ? this.findPackageRoot(importerPath, name) : undefined;

    // A package may refer to itself by name when it defines an exports map.
    let pkg = importerPackage?.name === name ? importerPackage : undefined;
    if (!pkg) {
      const searchFrom = importerPath ? this.options.path.dirname(importerPath) : this.options.rootDir;
      const packageRoot = this.findNodeModule(name, searchFrom);
      if (!packageRoot) {
        throw new NpmCompatibilityError(
          "MODULE_RESOLUTION",
          `Cannot resolve package ${specifier}${importerPath ? ` from ${importerPath}` : ""}`,
        );
      }
      pkg = readPackage(this.options.fs, this.options.path, packageRoot, name);
    }

    const { exports: exportsField } = pkg.packageJson;
    let target: string | undefined;
    if (exportsField !== undefined) {
      target = resolveExportTarget(exportsField, subpath);
      if (!target) {
        throw new NpmCompatibilityError(
          "MODULE_RESOLUTION",
          `Package ${pkg.name} does not export ${subpath}`,
        );
      }
    } else if (subpath !== ".") {
      target = subpath.slice(2);
    } else {
      const main = typeof pkg.packageJson.module === "string" ? pkg.packageJson.module : pkg.packageJson.main;
      target =
        typeof main === "string"
          ? main
          : this.options.fs.exists(this.options.path.join(pkg.root, "index.mjs"))
            ? "./index.mjs"
            : "./index.js";
    }

    return { package: pkg, filename: this.resolvePackageFile(pkg.root, target) };
  }

  resolveRelative(specifier: string, importer: ResolvedPackageModule): ResolvedPackageModule {
    if (!specifier.startsWith(".")) {
      throw new NpmCompatibilityError("MODULE_RESOLUTION", `Expected a relative import: ${specifier}`);
    }
    const candidate = this.options.path.resolve(this.options.path.dirname(importer.filename), specifier);
    const packageRelative = this.options.path.relative(importer.package.root, candidate);
    const filename = this.resolvePackageFile(importer.package.root, packageRelative);
    return { package: importer.package, filename };
  }

  private findPackageRoot(importerPath: string, requestedName: string): GuestPackage | undefined {
    // Keep package metadata alongside the entry if it is already a file from
    // the same package tree; self-reference then uses its exports map.
    let current = this.options.path.dirname(importerPath);
    while (true) {
      const packageJsonPath = this.options.path.join(current, "package.json");
      if (this.options.fs.exists(packageJsonPath)) {
        const json = parsePackageJson(this.options.fs.readText(packageJsonPath), packageJsonPath);
        if (json.name === requestedName) {
          return {
            name: requestedName,
            version: typeof json.version === "string" ? json.version : "0.0.0",
            root: this.options.fs.realpath(current) ?? current,
            packageJsonPath,
            packageJson: json,
          };
        }
      }
      const parent = this.options.path.dirname(current);
      if (parent === current) return undefined;
      current = parent;
    }
  }

  private findNodeModule(name: string, startingDirectory: string): string | undefined {
    let current = startingDirectory;
    while (true) {
      const packageRoot = this.options.path.join(current, "node_modules", ...name.split("/"));
      if (this.options.fs.exists(this.options.path.join(packageRoot, "package.json"))) {
        return this.options.fs.realpath(packageRoot) ?? packageRoot;
      }
      const parent = this.options.path.dirname(current);
      if (parent === current) return undefined;
      current = parent;
    }
  }

  private resolvePackageFile(packageRoot: string, target: string): string {
    if (target.startsWith("/") || target.startsWith("\\")) {
      throw new NpmCompatibilityError("MODULE_RESOLUTION", `Package target must be relative: ${target}`);
    }
    const normalizedTarget = target.startsWith("./") ? target.slice(2) : target;
    const base = this.options.path.resolve(packageRoot, normalizedTarget);
    const relative = this.options.path.relative(packageRoot, base);
    if (relative === ".." || relative.startsWith(`..${this.options.path.sep}`) || this.options.path.isAbsolute(relative)) {
      throw new NpmCompatibilityError("MODULE_RESOLUTION", `Package target escapes its root: ${target}`);
    }

    const ext = this.options.path.extname(base);
    const candidates = ext
      ? [base]
      : [
          ...[".mjs", ".js", ".jsx", ".ts", ".tsx"].map((suffix) => `${base}${suffix}`),
          ...["index.mjs", "index.js", "index.jsx", "index.ts", "index.tsx"].map((file) =>
            this.options.path.join(base, file),
          ),
          base,
        ];
    const realRoot = this.options.fs.realpath(packageRoot) ?? packageRoot;
    const filename = candidates
      .filter((candidate) => this.options.fs.exists(candidate))
      .map((candidate) => this.options.fs.realpath(candidate))
      .find((candidate): candidate is string => {
        if (!candidate) return false;
        const relativeToRoot = this.options.path.relative(realRoot, candidate);
        return (
          relativeToRoot !== ".." &&
          !relativeToRoot.startsWith(`..${this.options.path.sep}`) &&
          !this.options.path.isAbsolute(relativeToRoot)
        );
      });
    if (!filename) {
      throw new NpmCompatibilityError("MODULE_RESOLUTION", `Cannot resolve package file ${target} in ${packageRoot}`);
    }
    return filename;
  }
}
