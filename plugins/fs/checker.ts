/**
 * Filesystem permission enforcement for one loaded plugin.
 *
 * Effective permission = requested (manifest) ∩ host policy.
 *
 * Every privileged filesystem call runs through `FsPermissionChecker.resolve`,
 * which follows this order — and the order is the security property:
 *
 *   normalize separators → resolve `.`/`..` → resolve against plugin root
 *   → canonicalize (follow symlinks) → host policy → manifest permission
 *
 * Nothing is ever matched against the raw guest string.
 */

import * as nodePath from "node:path";

import { PermissionDeniedError } from "../core/errors";
import type { HostFileSystem } from "./host-filesystem";
import {
  compilePattern,
  escapesRoot,
  isAbsoluteGuestPath,
  matchRule,
  normalizeSegments,
  toPosix,
  type PathRule,
} from "./path-rules";

export type FsAccessMode = "read" | "write";

export interface CompiledFsPermissions {
  read: PathRule[];
  write: PathRule[];
}

/**
 * Substrate filesystem policy (absolute-path escape hatches). Consumed only
 * by the checker below — everything else is granted per capability name.
 */
export interface FsPolicyOptions {
  /** Allow reads outside the plugin root at all. */
  absoluteRead?: boolean;
  /** Allow writes outside the plugin root at all. Dangerous. */
  absoluteWrite?: boolean;
  /** Absolute glob patterns always denied, checked before everything else. */
  deny?: string[];
  /** When present, an out-of-root path must also match one of these. */
  allow?: string[];
}

/** The conservative default: plugins are confined to their own directory. */
export function defaultFsPolicy(): FsPolicyOptions {
  // Everything a plugin could reach outside itself is off by default.
  return { absoluteRead: false, absoluteWrite: false };
}

export interface CompiledFsPolicy {
  absoluteRead: boolean;
  absoluteWrite: boolean;
  deny: PathRule[];
  allow: PathRule[] | null;
}

function compilePolicyPatterns(patterns: string[] | undefined, field: string): PathRule[] | null {
  if (patterns === undefined) return null;
  return patterns.map((pattern) => compilePattern(pattern, field));
}

export function compileFsPolicy(fsPolicy: FsPolicyOptions = {}): CompiledFsPolicy {
  return {
    absoluteRead: fsPolicy.absoluteRead ?? false,
    absoluteWrite: fsPolicy.absoluteWrite ?? false,
    deny: compilePolicyPatterns(fsPolicy.deny, "policy.fs.deny") ?? [],
    allow: compilePolicyPatterns(fsPolicy.allow, "policy.fs.allow"),
  };
}

function nativeToPosix(nativePath: string): string {
  return toPosix(nativePath);
}

function isInside(root: string, candidate: string): boolean {
  if (candidate === root) return true;
  const prefix = root.endsWith(nodePath.sep) ? root : root + nodePath.sep;
  return candidate.startsWith(prefix);
}

export interface ResolvedPath {
  /** Canonical native path, safe to hand to the filesystem backend. */
  native: string;
  /** True when the canonical path lives inside the plugin root. */
  insideRoot: boolean;
}

/**
 * Resolves and authorizes guest paths for one loaded plugin.
 *
 * A single instance is shared by every capability function of that plugin.
 */
export class FsPermissionChecker {
  private readonly canonicalRoot: string;

  constructor(
    root: string,
    private readonly permissions: CompiledFsPermissions,
    private readonly policy: CompiledFsPolicy,
    private readonly fs: HostFileSystem,
  ) {
    const resolvedRoot = nodePath.resolve(root);
    this.canonicalRoot = this.fs.realpath(resolvedRoot) ?? resolvedRoot;
  }

  /** The canonical plugin root. Host-side only — never handed to the guest. */
  get rootPath(): string {
    return this.canonicalRoot;
  }

  /**
   * Canonicalize a native path that may not exist yet: resolve the longest
   * existing prefix through `realpath`, then re-append the missing tail.
   * Segments that do not exist cannot be symlinks, so this is safe for writes
   * that create a new file.
   */
  private canonicalize(nativePath: string): string {
    let current = nodePath.resolve(nativePath);
    const tail: string[] = [];
    for (;;) {
      const real = this.fs.realpath(current);
      if (real !== null) {
        return tail.length === 0 ? real : nodePath.join(real, ...tail);
      }
      const parent = nodePath.dirname(current);
      if (parent === current) {
        return nodePath.join(current, ...tail);
      }
      tail.unshift(nodePath.basename(current));
      current = parent;
    }
  }

  /**
   * Resolve a guest path and authorize it for `mode`, or throw
   * `PermissionDeniedError`.
   */
  resolve(requested: unknown, mode: FsAccessMode): ResolvedPath {
    if (typeof requested !== "string" || requested === "") {
      throw new PermissionDeniedError(`fs.${mode} requires a non-empty path string`);
    }
    if (requested.includes("\0")) {
      throw new PermissionDeniedError("path contains a NUL byte");
    }

    const posix = toPosix(requested);
    const absolute = isAbsoluteGuestPath(posix);
    const drive = absolute && /^[A-Za-z]:\//.test(posix) ? posix.slice(0, 2) : "";
    const normalized = normalizeSegments(drive ? posix.slice(2) : posix, absolute);

    if (!absolute && escapesRoot(normalized)) {
      throw new PermissionDeniedError("path escapes plugin root");
    }

    const native = absolute
      ? nodePath.resolve(`${drive}/${normalized}`)
      : nodePath.resolve(this.canonicalRoot, normalized.split("/").join(nodePath.sep));

    const canonical = this.canonicalize(native);
    const insideRoot = isInside(this.canonicalRoot, canonical);

    // A relative request that lands outside the root got there through a
    // symlink: refuse without revealing where it pointed.
    if (!absolute && !insideRoot) {
      throw new PermissionDeniedError("path escapes plugin root");
    }

    this.checkPolicy(canonical, insideRoot, mode);
    this.checkManifest(requested, canonical, insideRoot, mode);

    return { native: canonical, insideRoot };
  }

  /** Host policy: the final authority, checked before manifest permissions. */
  private checkPolicy(canonical: string, insideRoot: boolean, mode: FsAccessMode): void {
    const canonicalPosix = nativeToPosix(canonical);

    for (const rule of this.policy.deny) {
      if (matchRule(rule, canonicalPosix)) {
        throw new PermissionDeniedError("path is outside allowed scope");
      }
    }

    if (insideRoot) return;

    const allowed =
      mode === "read" ? this.policy.absoluteRead : this.policy.absoluteWrite;
    if (!allowed) {
      throw new PermissionDeniedError(
        `absolute filesystem ${mode}s are disabled by host policy`,
      );
    }

    const allowList = this.policy.allow;
    if (allowList && !allowList.some((rule) => matchRule(rule, canonicalPosix))) {
      throw new PermissionDeniedError("path is outside allowed scope");
    }
  }

  /** Manifest permissions, matched against the canonical path only. */
  private checkManifest(
    requested: string,
    canonical: string,
    insideRoot: boolean,
    mode: FsAccessMode,
  ): void {
    const rules = this.permissions[mode];
    const relative = insideRoot
      ? nativeToPosix(nodePath.relative(this.canonicalRoot, canonical))
      : null;
    const canonicalPosix = nativeToPosix(canonical);

    for (const rule of rules) {
      if (rule.kind === "all") return;
      if (rule.kind === "relative") {
        if (relative !== null && matchRule(rule, relative)) return;
        continue;
      }
      if (matchRule(rule, canonicalPosix)) return;
    }

    throw new PermissionDeniedError(
      `fs.${mode} is not permitted for ${JSON.stringify(requested)}`,
    );
  }
}
