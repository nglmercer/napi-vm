/**
 * Filesystem permission enforcement for one loaded plugin.
 *
 * Effective permission = requested (manifest) ∩ host policy.
 *
 * Every privileged filesystem call runs through `FsPermissionChecker.resolve`,
 * which follows this order — and the order is the security property:
 *
 *   normalize separators → resolve `.`/`..` → resolve against plugin root
 *   → canonicalize (follow symlinks) → confinement gate → manifest permission
 *
 * Nothing is ever matched against the raw guest string.
 */

import { PermissionDeniedError } from "../core/errors";
import { posixPath, type HostFileSystem, type HostPath } from "../platform";
import {
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

function nativeToPosix(nativePath: string): string {
  return toPosix(nativePath);
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
 * Native path arithmetic goes through the injected `HostPath` (the host
 * passes its platform paths; direct constructions default to POSIX, which is
 * exact on POSIX hosts).
 */
export class FsPermissionChecker {
  private readonly canonicalRoot: string;
  private readonly path: HostPath;
  private readonly fs: HostFileSystem;
  private readonly permissions: CompiledFsPermissions;

  constructor(
    root: string,
    permissions: CompiledFsPermissions,
    fs: HostFileSystem,
    path: HostPath = posixPath,
  ) {
    this.path = path;
    this.fs = fs;
    this.permissions = permissions;
    const resolvedRoot = path.resolve(root);
    this.canonicalRoot = fs.realpath(resolvedRoot) ?? resolvedRoot;
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
    let current = this.path.resolve(nativePath);
    const tail: string[] = [];
    for (;;) {
      const real = this.fs.realpath(current);
      if (real !== null) {
        return tail.length === 0 ? real : this.path.join(real, ...tail);
      }
      const parent = this.path.dirname(current);
      if (parent === current) {
        return this.path.join(current, ...tail);
      }
      tail.unshift(this.path.basename(current));
      current = parent;
    }
  }

  private isInside(candidate: string): boolean {
    if (candidate === this.canonicalRoot) return true;
    const prefix = this.canonicalRoot.endsWith(this.path.sep)
      ? this.canonicalRoot
      : this.canonicalRoot + this.path.sep;
    return candidate.startsWith(prefix);
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
      ? this.path.resolve(`${drive}/${normalized}`)
      : this.path.resolve(this.canonicalRoot, normalized.split("/").join(this.path.sep));

    const canonical = this.canonicalize(native);
    const insideRoot = this.isInside(canonical);

    // A relative request that lands outside the root got there through a
    // symlink: refuse without revealing where it pointed.
    if (!absolute && !insideRoot) {
      throw new PermissionDeniedError("path escapes plugin root");
    }

    this.checkPolicy(insideRoot, mode);
    this.checkManifest(requested, canonical, insideRoot, mode);

    return { native: canonical, insideRoot };
  }

  /**
   * Confinement gate: nothing outside the plugin root is reachable, ever.
   * Checked before manifest permissions, so an absolute manifest grant can
   * only ever match paths inside the root.
   */
  private checkPolicy(insideRoot: boolean, mode: FsAccessMode): void {
    if (!insideRoot) {
      throw new PermissionDeniedError(
        `absolute filesystem ${mode}s are outside the plugin root`,
      );
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
      ? nativeToPosix(this.path.relative(this.canonicalRoot, canonical))
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
