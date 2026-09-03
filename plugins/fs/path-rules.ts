/**
 * Guest path normalization and the small glob dialect used by plugin
 * permissions.
 *
 * Guest paths are always POSIX-style (`a/b/c`), whatever the host runs on.
 * Backslashes are folded to `/` before anything else so a Windows-style
 * `..\..\etc` can never sneak past the traversal checks.
 *
 * Glob dialect — deliberately tiny:
 *
 *   `*`   matches within a single path segment
 *   `**`  matches zero or more whole segments
 *
 * Everything else is literal.
 */

import { PluginManifestError } from "../core/errors";

export type PathRuleKind = "all" | "relative" | "absolute";

export interface PathRule {
  kind: PathRuleKind;
  /** Source pattern, kept for diagnostics. `"*"` for `kind: "all"`. */
  pattern: string;
  /** Compiled matcher; absent for `kind: "all"`, which matches everything. */
  regex?: RegExp;
}

/** Fold separators and reject strings that can never be a path. */
export function toPosix(input: string): string {
  return input.replace(/\\/g, "/");
}

/** True for `/abs`, `C:/abs` and `//server/share`. */
export function isAbsoluteGuestPath(posixPath: string): boolean {
  return posixPath.startsWith("/") || /^[A-Za-z]:\//.test(posixPath);
}

/**
 * Resolve `.` and `..` textually. Leading `..` segments are preserved so the
 * caller can detect an escape attempt; absolute paths drop them at the root.
 */
export function normalizeSegments(posixPath: string, absolute: boolean): string {
  const out: string[] = [];
  for (const segment of posixPath.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") {
      const last = out[out.length - 1];
      if (last !== undefined && last !== "..") out.pop();
      else if (!absolute) out.push("..");
      continue;
    }
    out.push(segment);
  }
  return out.join("/");
}

/**
 * True when a normalized relative path escapes its root.
 *
 * Only a literal `..` segment escapes. Testing `startsWith("..")` instead also
 * rejects ordinary names that merely begin with two dots -- `..cache`,
 * `..data` -- which are legal filenames on every platform this runs on. Every
 * caller shares this one definition so the permission compiler and the runtime
 * resolver cannot disagree about what an escape is.
 */
export function escapesRoot(normalized: string): boolean {
  return normalized === ".." || normalized.startsWith("../");
}

const REGEX_META = /[.+^${}()|[\]\\]/g;

function segmentToRegex(segment: string): string {
  return segment
    .split("*")
    .map((chunk) => chunk.replace(REGEX_META, "\\$&"))
    .join("[^/]*");
}

/**
 * Compile pattern segments into a regex matched against a path that is
 * *always* prefixed with `/`. That uniform leading separator is what lets
 * `**` stand for "zero or more segments" without special-casing the head.
 */
function segmentsToRegex(segments: string[]): RegExp {
  let source = "";
  segments.forEach((segment, index) => {
    const isLast = index === segments.length - 1;
    if (segment === "**") {
      // `a/**` covers `a` itself and everything beneath it.
      source += isLast ? "(?:/.*)?" : "(?:/[^/]*)*";
      return;
    }
    source += `/${segmentToRegex(segment)}`;
  });
  return new RegExp(`^${source}$`);
}

/**
 * Compile one manifest pattern into a rule.
 *
 * `field` names the manifest location (`permissions.fs.read`) and is only used
 * for error messages.
 */
export function compilePattern(pattern: string, field: string): PathRule {
  if (pattern.includes("\0")) {
    throw new PluginManifestError(`${field} contains a NUL byte`);
  }
  const posix = toPosix(pattern).trim();
  if (posix === "") {
    throw new PluginManifestError(`${field} contains an empty pattern`);
  }
  if (posix === "*" || posix === "**") {
    return { kind: "all", pattern: "*" };
  }

  const absolute = isAbsoluteGuestPath(posix);
  const drive = absolute && /^[A-Za-z]:\//.test(posix) ? posix.slice(0, 2) : "";
  const body = drive ? posix.slice(2) : posix;
  const normalized = normalizeSegments(body, absolute);

  if (!absolute && escapesRoot(normalized)) {
    throw new PluginManifestError(
      `${field} pattern "${pattern}" escapes the plugin root`,
    );
  }
  if (normalized === "") {
    throw new PluginManifestError(
      `${field} pattern "${pattern}" resolves to an empty path`,
    );
  }

  const segments = normalized.split("/");
  return {
    kind: absolute ? "absolute" : "relative",
    pattern: absolute ? `${drive}/${normalized}` : normalized,
    regex: segmentsToRegex(drive ? [drive.toLowerCase(), ...segments] : segments),
  };
}

/**
 * Match a candidate path against a compiled rule.
 *
 * `candidate` is a plugin-root-relative POSIX path for `relative` rules and an
 * absolute POSIX path for `absolute` rules — in both cases already normalized
 * and canonicalized by the caller. Never call this with a raw guest string.
 */
/**
 * Validate a manifest entry path: it must be a relative POSIX path that
 * stays inside the plugin directory. Returns the normalized form.
 *
 * This lives here (not in `manifest.ts`) so the manifest module stays
 * agnostic to paths: the host calls it after parsing. The canonical
 * containment check still happens on the real path in the host — this is
 * the fail-fast textual gate with the same error wording.
 */
export function validateEntryPath(value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new PluginManifestError("entry must be a non-empty string");
  }
  const posix = toPosix(value);
  if (isAbsoluteGuestPath(posix)) {
    throw new PluginManifestError("entry must be a path inside the plugin directory");
  }
  const normalized = normalizeSegments(posix, false);
  if (normalized === "" || escapesRoot(normalized)) {
    throw new PluginManifestError("entry must be a path inside the plugin directory");
  }
  return normalized;
}

export function matchRule(rule: PathRule, candidate: string): boolean {
  if (rule.kind === "all") return true;
  if (!rule.regex) return false;
  const subject =
    rule.kind === "absolute"
      ? candidate.replace(/^([A-Za-z]):/, (_m, d: string) => `/${d.toLowerCase()}:`)
      : `/${candidate}`;
  return rule.regex.test(subject);
}
