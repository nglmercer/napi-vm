/**
 * Portable platform: everything the plugin core needs from the outside world.
 *
 * This module imports nothing — no `node:*`, no npm packages — so the plugin
 * core (`core/`, `capabilities/`, `fs/`) loads identically on Node, Bun, Deno,
 * a Tauri/Electron renderer, or any bundler output. Hosts that run on Node
 * get a ready-made platform from `"napi-vm/plugins/node"` (`nodePlatform()`);
 * everyone else assembles one from their own filesystem, path helpers and
 * crypto source.
 *
 * ```ts
 * import { PluginHost, portablePlatform, posixPath, portableCrypto } from "napi-vm/plugins";
 *
 * const host = new PluginHost({
 *   policy,
 *   platform: portablePlatform(myTauriFileSystem),
 * });
 * ```
 */

/** Filesystem backend. All paths are native, already-permitted paths. */
export interface HostFileSystem {
  /**
   * Fully resolved real path (symlinks followed), or `null` when the path
   * does not exist. Any other failure must throw.
   */
  realpath(nativePath: string): string | null;
  readText(nativePath: string): string;
  writeText(nativePath: string, contents: string): void;
  exists(nativePath: string): boolean;
}

/**
 * Path helpers in one flavor. The checker and the host resolve *native* paths
 * through this, so a Node host passes its native `node:path` wrapper while a
 * portable host passes {@link posixPath}. Guest-visible helpers (`napi:path`)
 * always use {@link posixPath} directly: guest paths are POSIX on every host.
 */
export interface HostPath {
  /** Native segment separator (`"/"` on POSIX, `"\\"` on Windows). */
  readonly sep: string;
  /** Working directory used by {@link resolve} for relative segments. */
  cwd(): string;
  /** Like `node:path.resolve`: absolute wins, otherwise resolved under `cwd`. */
  resolve(...parts: string[]): string;
  /** Like `node:path.join`. */
  join(...parts: string[]): string;
  /** Like `node:path.normalize`. */
  normalize(path: string): string;
  /** Like `node:path.dirname`. */
  dirname(path: string): string;
  /** Like `node:path.basename`, with the optional extension stripped. */
  basename(path: string, ext?: string): string;
  /** Like `node:path.extname`. */
  extname(path: string): string;
  /** Like `node:path.relative`. */
  relative(from: string, to: string): string;
  /** True for `/abs` paths (POSIX flavor; the node wrapper covers the rest). */
  isAbsolute(path: string): boolean;
}

/** Synchronous cryptographic source for `napi:crypto`. */
export interface HostCrypto {
  /** `count` cryptographically random bytes. */
  randomBytes(count: number): Uint8Array;
  /** An RFC 4122 v4 UUID string. */
  randomUUID(): string;
  /** Hex digest of `data`; `algorithm` is pre-checked against the allowlist. */
  digest(algorithm: string, data: Uint8Array): string;
}

/**
 * The whole outside world, as one value. `fs` is the only part every host
 * must supply; `path`/`crypto` default to the portable implementations and
 * `requireNative` stays absent unless the host explicitly offers it.
 */
export interface HostPlatform {
  fs: HostFileSystem;
  path: HostPath;
  crypto: HostCrypto;
  /**
   * Load a host-side module by specifier (e.g. `"miniaudio_node"`). Absent on
   * platforms without a module loader — capabilities that need one (audio's
   * default player) fail with a message pointing at the grant-provided
   * factory instead.
   */
  requireNative?: (specifier: string) => unknown;
}

/**
 * Default cap on a single `readText`/`writeText`, in bytes.
 *
 * Permission to read a path is not permission to spend unbounded host memory
 * on it. The VM's own 16 MiB string ceiling only rejects the value *after*
 * the host has allocated the whole file, so a permitted 2 GiB file would
 * still be read, marshalled and only then refused. 8 MiB leaves headroom
 * under that ceiling while staying far above any plausible plugin source or
 * config file.
 */
export const DEFAULT_MAX_FILE_BYTES = 8 * 1024 * 1024;

// ── pure POSIX path ──────────────────────────────────────────────────

function stripTrailingSlashes(path: string): string {
  let end = path.length;
  while (end > 1 && path[end - 1] === "/") end--;
  return path.slice(0, end);
}

/** Last path segment with trailing slashes ignored (`""` when there is none). */
function plainBasename(path: string): string {
  const stripped = stripTrailingSlashes(path);
  if (stripped === "") return "";
  const slash = stripped.lastIndexOf("/");
  return slash === -1 ? stripped : stripped.slice(slash + 1);
}

/**
 * Pure POSIX path helpers. No host calls, no locale, no loader — safe to use
 * for guest-visible computation on every platform.
 */
export const posixPath: HostPath = {
  sep: "/",

  cwd: () => "/",

  isAbsolute: (path) => path.startsWith("/"),

  normalize(path: string): string {
    if (path === "") return ".";
    const absolute = path.startsWith("/");
    const trailingSlash = path.length > 1 && path.endsWith("/");
    const out: string[] = [];
    for (const segment of path.split("/")) {
      if (segment === "" || segment === ".") continue;
      if (segment === "..") {
        const last = out[out.length - 1];
        if (last !== undefined && last !== "..") out.pop();
        else if (!absolute) out.push("..");
        continue;
      }
      out.push(segment);
    }
    let result = (absolute ? "/" : "") + out.join("/");
    if (result === "") result = absolute ? "/" : ".";
    if (trailingSlash && result !== "/" && !result.endsWith("/")) result += "/";
    return result;
  },

  join(...parts: string[]): string {
    const nonEmpty = parts.filter((part) => part !== "");
    if (nonEmpty.length === 0) return ".";
    return posixPath.normalize(nonEmpty.join("/"));
  },

  resolve(...parts: string[]): string {
    let resolved = "";
    let foundAbsolute = false;
    for (let i = parts.length - 1; i >= 0; i--) {
      const part = parts[i];
      if (part === "") continue;
      resolved = resolved === "" ? part : `${part}/${resolved}`;
      if (part.startsWith("/")) {
        foundAbsolute = true;
        break;
      }
    }
    if (!foundAbsolute) {
      const cwd = posixPath.cwd();
      resolved = resolved === "" ? cwd : `${cwd}/${resolved}`;
    }
    return posixPath.normalize(resolved);
  },

  dirname(path: string): string {
    if (path === "") return ".";
    const stripped = stripTrailingSlashes(path);
    const slash = stripped.lastIndexOf("/");
    if (slash === -1) return ".";
    if (slash === 0) return "/";
    return stripped.slice(0, slash);
  },

  basename(path: string, ext?: string): string {
    if (ext === undefined || ext === "" || ext.length > path.length) {
      return plainBasename(path);
    }
    if (ext === path) return "";
    const core = stripTrailingSlashes(path);
    // An all-slash path with an unmatched suffix comes back raw.
    if (core === "" || /^\/+$/.test(core)) return path;
    let base = core;
    // Never strip down to an empty or all-slash remainder: the suffix stays.
    if (base.length > ext.length && base.endsWith(ext)) {
      const remainder = base.slice(0, base.length - ext.length);
      if (remainder !== "" && !/^\/+$/.test(remainder)) base = remainder;
    }
    return plainBasename(base);
  },

  extname(path: string): string {
    const base = posixPath.basename(path);
    // A bare "." or ".." has no extension; longer dot-runs do ("..." → ".").
    if (base === "." || base === "..") return "";
    const dot = base.lastIndexOf(".");
    if (dot <= 0) return "";
    return base.slice(dot);
  },

  relative(from: string, to: string): string {
    const fromResolved = posixPath.resolve(from);
    const toResolved = posixPath.resolve(to);
    if (fromResolved === toResolved) return "";
    const fromSegments = fromResolved.split("/").filter((s) => s !== "");
    const toSegments = toResolved.split("/").filter((s) => s !== "");
    let common = 0;
    while (
      common < fromSegments.length &&
      common < toSegments.length &&
      fromSegments[common] === toSegments[common]
    ) {
      common++;
    }
    const up = fromSegments.length - common;
    return [...Array<string>(up).fill(".."), ...toSegments.slice(common)].join("/");
  },
};

// ── portable crypto ──────────────────────────────────────────────────

function webCrypto(): Crypto | undefined {
  return (globalThis as { crypto?: Crypto }).crypto;
}

/**
 * Crypto source built only on WebCrypto (`globalThis.crypto`), which exists
 * on Node 18+, Bun, Deno and browsers. Synchronous digests are impossible
 * through WebCrypto, so `digest` throws a plain error here — hosts that need
 * `digest()` supply their own {@link HostCrypto} (the Node platform ships
 * one backed by `node:crypto`).
 */
export function portableCrypto(): HostCrypto {
  return {
    randomBytes(count: number): Uint8Array {
      const crypto = webCrypto();
      if (!crypto) {
        throw new Error("portableCrypto needs globalThis.crypto for randomness");
      }
      const out = new Uint8Array(count);
      crypto.getRandomValues(out);
      return out;
    },
    randomUUID(): string {
      const crypto = webCrypto();
      if (crypto?.randomUUID) return crypto.randomUUID();
      // RFC 4122 v4 fallback when `randomUUID` is missing but the RNG exists.
      const bytes = portableCrypto().randomBytes(16);
      bytes[6] = (bytes[6] & 0x0f) | 0x40;
      bytes[8] = (bytes[8] & 0x3f) | 0x80;
      const hex = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
      return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
    },
    digest(): string {
      throw new Error(
        "digest() is not available on this platform: supply a HostCrypto " +
          "(e.g. nodeCrypto from \"napi-vm/plugins/node\") or withhold napi:crypto",
      );
    },
  };
}

// ── assembly ─────────────────────────────────────────────────────────

/** A filesystem backend that fails every call with a wiring error. */
export function missingFileSystem(callee = "PluginHost"): HostFileSystem {
  const fail = (op: string): never => {
    throw new Error(
      `${callee} has no filesystem: pass \`platform: nodePlatform()\` ` +
        `from "napi-vm/plugins/node" (or a custom HostFileSystem) to load plugins from disk`,
    );
  };
  return {
    realpath: () => fail("realpath"),
    readText: () => fail("readText"),
    writeText: () => fail("writeText"),
    exists: () => fail("exists"),
  };
}

/**
 * Assemble a platform from a filesystem plus portable defaults (POSIX paths,
 * WebCrypto randomness). Desktop/Tauri hosts pass their own `HostFileSystem`;
 * Node hosts use `nodePlatform()` instead.
 */
export function portablePlatform(fs: HostFileSystem): HostPlatform {
  return { fs, path: posixPath, crypto: portableCrypto() };
}
