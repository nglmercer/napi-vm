import { NpmCompatibilityError } from "./types";

export interface GuestPackageJson {
  name?: string;
  version?: string;
  type?: string;
  main?: string;
  module?: string;
  exports?: unknown;
  dependencies?: Record<string, string>;
  [key: string]: unknown;
}

export function parsePackageJson(source: string, filename: string): GuestPackageJson {
  let value: unknown;
  try {
    value = JSON.parse(source);
  } catch (cause) {
    throw new NpmCompatibilityError(
      "MODULE_RESOLUTION",
      `Invalid package.json at ${filename}: ${cause instanceof Error ? cause.message : String(cause)}`,
      { cause },
    );
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new NpmCompatibilityError("MODULE_RESOLUTION", `package.json at ${filename} must contain an object`);
  }
  return value as GuestPackageJson;
}

function resolveTarget(value: unknown): string | undefined {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    for (const candidate of value) {
      const resolved = resolveTarget(candidate);
      if (resolved) return resolved;
    }
    return undefined;
  }
  if (typeof value !== "object" || value === null) return undefined;

  const record = value as Record<string, unknown>;
  if (Object.keys(record).some((key) => key.startsWith("."))) return undefined;
  // These are the conditions the guest loader promises to understand. Other
  // conditions, including `require`, are intentionally not selected.
  for (const condition of ["import", "node", "default"]) {
    if (condition in record) {
      const resolved = resolveTarget(record[condition]);
      if (resolved) return resolved;
    }
  }
  return undefined;
}

function matchExportPattern(exportsMap: Record<string, unknown>, subpath: string): unknown {
  if (subpath in exportsMap) return exportsMap[subpath];
  const patterns = Object.keys(exportsMap)
    .filter((key) => key.includes("*"))
    .map((key) => {
      const [prefix, suffix = ""] = key.split("*", 2);
      return { key, prefix, suffix };
    })
    .filter(({ prefix, suffix }) => subpath.startsWith(prefix) && subpath.endsWith(suffix))
    .sort((a, b) => b.prefix.length - a.prefix.length || b.suffix.length - a.suffix.length);
  const match = patterns[0];
  if (!match) return undefined;
  const replacement = subpath.slice(match.prefix.length, subpath.length - match.suffix.length);
  const replace = (target: unknown): unknown => {
    if (typeof target === "string") return target.replaceAll("*", replacement);
    if (Array.isArray(target)) return target.map(replace);
    if (typeof target === "object" && target !== null) {
      return Object.fromEntries(Object.entries(target).map(([key, value]) => [key, replace(value)]));
    }
    return target;
  };
  return replace(exportsMap[match.key]);
}

/** Resolve an ESM export target, honoring `import`, `node`, and `default`. */
export function resolveExportTarget(exportsField: unknown, subpath: string): string | undefined {
  if (typeof exportsField === "string" || Array.isArray(exportsField)) {
    return subpath === "." ? resolveTarget(exportsField) : undefined;
  }
  if (typeof exportsField !== "object" || exportsField === null) return undefined;

  const exportsMap = exportsField as Record<string, unknown>;
  const hasSubpathKeys = Object.keys(exportsMap).some((key) => key.startsWith("."));
  if (!hasSubpathKeys) return subpath === "." ? resolveTarget(exportsMap) : undefined;
  return resolveTarget(matchExportPattern(exportsMap, subpath));
}
