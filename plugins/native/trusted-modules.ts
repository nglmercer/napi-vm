/**
 * Trusted native packages: the operator-side half of dynamic modules.
 *
 * Downloading and exposing code is NEVER a guest decision. The flow is:
 *
 *   1. A known, trusted operator pins `{ package, version, integrity }`.
 *   2. `installTrustedPackage` checks the package against the allowlist,
 *      downloads the tarball into the modules folder, verifies the
 *      `sha512-…` integrity, extracts it, and `require`s it on the HOST.
 *   3. `nativePackageCapability` wraps the loaded code in a closed
 *      allowlist (`native-bridge.ts`) and registers it with
 *      `defineCapability`, so plugins request it by name and the host
 *      policy grants or denies it like any other capability.
 *
 * Fail-closed rules: exact pinned versions only (no ranges, no `latest`),
 * integrity required by default, path traversal inside tarballs refused,
 * and anything outside the allowlist is refused before any network happens.
 */

import { createHash, timingSafeEqual } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";

import { PluginLoadError } from "../core/errors";
import {
  defineCapability,
  type CapabilityDefinition,
} from "../capabilities/capability-registry";
import type { FetchTransport } from "../capabilities/fetch-capability";
import {
  installNativeModule,
  type NativeModuleDefinition,
} from "../core/native-bridge";

export const DEFAULT_REGISTRY = "https://registry.npmjs.org";
export const DEFAULT_MODULES_DIR = ".napi-vm/modules";

/** Largest tarball accepted, in bytes. npm tarballs are small; refuse blobs. */
export const MAX_TARBALL_BYTES = 64 * 1024 * 1024;
/** Largest extracted file count. A tarbomb with 100k entries stops here. */
export const MAX_TARBALL_FILES = 4096;

/** Operator policy: where modules live and which packages may enter. */
export interface TrustedModulesPolicy {
  /** Folder for verified packages. Created on first install. */
  dir?: string;
  /** Registries tried in order. Defaults to npmjs. `https:` only. */
  registries?: string[];
  /**
   * Allowlist: exact names (`"miniaudio_node"`) or scope prefixes
   * (`"@myorg/*"`). Anything else is refused before any network happens.
   */
  allow: string[];
  /** Refuse specs without a pinned integrity. Defaults to `true`. */
  requireIntegrity?: boolean;
}

/** One pinned package: exact version, integrity hash, nothing floating. */
export interface TrustedPackageSpec {
  package: string;
  version: string;
  /** `"sha512-<base64>"` (or sha384/sha256). Required unless the policy opts out. */
  integrity?: string;
}

export interface LoadedTrustedPackage {
  name: string;
  version: string;
  /** Install directory (`<dir>/<name>@<version>/`). */
  dir: string;
  /** Resolved host entry file that was `require`d. */
  entry: string;
  exports: Record<string, unknown>;
}

const PACKAGE_PATTERN = /^(?:@[a-z0-9-~][a-z0-9-._~]*\/)?[a-z0-9-~][a-z0-9-._~]*$/;
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const INTEGRITY_PATTERN = /^(sha512|sha384|sha256)-([A-Za-z0-9+/]+=*)$/;

function isAllowed(name: string, allow: string[]): boolean {
  return allow.some((pattern) => {
    if (pattern.endsWith("/*")) {
      const scope = pattern.slice(0, -1);
      return name.startsWith(scope) && name.length > scope.length;
    }
    return name === pattern;
  });
}

/** Validate the spec — name, pinned version, allowlist, integrity presence. */
export function assertTrustedSpec(policy: TrustedModulesPolicy, spec: TrustedPackageSpec): void {
  if (!PACKAGE_PATTERN.test(spec.package)) {
    throw new PluginLoadError(`untrusted package name "${spec.package}"`);
  }
  if (!VERSION_PATTERN.test(spec.version)) {
    throw new PluginLoadError(
      `package "${spec.package}" must pin an exact version, got "${spec.version}"`,
    );
  }
  if (!isAllowed(spec.package, policy.allow)) {
    throw new PluginLoadError(`package "${spec.package}" is not in the trusted allowlist`);
  }
  if ((policy.requireIntegrity ?? true) && !spec.integrity) {
    throw new PluginLoadError(
      `package "${spec.package}" needs a pinned integrity hash (sha512-…)`,
    );
  }
  if (spec.integrity !== undefined && !INTEGRITY_PATTERN.test(spec.integrity.trim())) {
    throw new PluginLoadError(`package "${spec.package}" has a malformed integrity hash`);
  }
}

/** npm tarball URL for an exact package@version on a registry. */
export function packageTarballUrl(registry: string, name: string, version: string): string {
  const clean = registry.replace(/\/+$/, "");
  // Plain HTTP is refused everywhere except loopback (local test mirrors),
  // the same exception Go makes for localhost.
  const loopback = /^http:\/\/(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/.test(clean);
  if (!clean.startsWith("https://") && !loopback) {
    throw new PluginLoadError(`registry must be https, got "${registry}"`);
  }
  const base = name.includes("/") ? name.slice(name.lastIndexOf("/") + 1) : name;
  return `${clean}/${name}/-/${base}-${version}.tgz`;
}

/** Create the modules folder (and parents). Returns the resolved path. */
export function ensureModulesDir(dir: string): string {
  mkdirSync(dir, { recursive: true });
  return realpathSync(dir);
}

/** Compare `data` against a `"sha512-<base64>"` integrity string. Throws on mismatch. */
export function verifyIntegrity(data: Uint8Array, integrity: string): void {
  const match = INTEGRITY_PATTERN.exec(integrity.trim());
  if (!match) throw new PluginLoadError("integrity must look like \"sha512-<base64>\"");
  const digest = createHash(match[1]).update(data).digest("base64");
  const actual = Buffer.from(digest);
  const expected = Buffer.from(match[2]);
  if (actual.length !== expected.length || !timingSafeEqual(actual, expected)) {
    throw new PluginLoadError("tarball integrity mismatch: refused");
  }
}

// ── minimal ustar extraction (no dependency, works in bun/node/compile) ──

function readCString(block: Uint8Array, offset: number, length: number): string {
  let end = offset;
  while (end < offset + length && block[end] !== 0) end++;
  return new TextDecoder().decode(block.subarray(offset, end));
}

function readOctal(block: Uint8Array, offset: number, length: number, field: string): number {
  const text = readCString(block, offset, length).trim();
  if (!/^[0-7]*$/.test(text)) {
    throw new PluginLoadError(`tarball has a malformed ${field} header`);
  }
  return text === "" ? 0 : parseInt(text, 8);
}

/** Refuse absolute paths, `..` escapes, NUL bytes and empty names. */
export function assertSafeTarPath(rawName: string): void {
  if (rawName === "" || rawName.includes("\0")) {
    throw new PluginLoadError("tarball has an invalid entry name");
  }
  const posix = rawName.replace(/\\/g, "/");
  if (
    posix.startsWith("/") ||
    /^[A-Za-z]:\//.test(posix) ||
    posix.split("/").some((segment) => segment === "..")
  ) {
    throw new PluginLoadError(`tarball entry escapes its directory: "${rawName}"`);
  }
}

/**
 * Extract a `.tgz`-inner tar (already gunzipped bytes) into `destDir`.
 * Supports regular files, directories, `pax` headers and GNU long names;
 * links, devices and everything else are skipped. Returns entry names.
 */
export function extractTarball(data: Uint8Array, destDir: string): string[] {
  const written: string[] = [];
  let offset = 0;
  let pendingLongName: string | null = null;
  let files = 0;
  while (offset + 512 <= data.length) {
    const header = data.subarray(offset, offset + 512);
    offset += 512;
    let allZero = true;
    for (const byte of header) {
      if (byte !== 0) {
        allZero = false;
        break;
      }
    }
    if (allZero) break;
    const name = readCString(header, 0, 100);
    const size = readOctal(header, 124, 12, "size");
    const typeflag = String.fromCharCode(header[156]);
    const prefix = readCString(header, 345, 155);
    if (offset + size > data.length) {
      throw new PluginLoadError("tarball is truncated");
    }
    const payload = data.subarray(offset, offset + size);
    offset += Math.ceil(size / 512) * 512;
    if (typeflag === "L" || typeflag === "K") {
      pendingLongName = new TextDecoder().decode(payload).replace(/\0.*$/s, "");
      continue;
    }
    const rawName = pendingLongName ?? (prefix !== "" ? `${prefix}/${name}` : name);
    pendingLongName = null;
    if (typeflag === "x" || typeflag === "g") continue;
    assertSafeTarPath(rawName);
    if (++files > MAX_TARBALL_FILES) {
      throw new PluginLoadError("tarball has too many entries: refused");
    }
    const native = join(destDir, ...rawName.split("/"));
    if (typeflag === "5") {
      mkdirSync(native, { recursive: true });
      continue;
    }
    if (typeflag !== "0" && typeflag !== "\0") continue;
    mkdirSync(dirname(native), { recursive: true });
    writeFileSync(native, payload);
    written.push(rawName);
  }
  return written;
}

// ── install ──────────────────────────────────────────────────────────

async function gunzip(data: Uint8Array): Promise<Uint8Array> {
  const stream = new DecompressionStream("gzip");
  const writer = stream.writable.getWriter();
  const reader = stream.readable.getReader();
  const chunks: Uint8Array[] = [];
  const pump = (async () => {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
    }
  })();
  await writer.write(new Uint8Array(data));
  await writer.close();
  await pump;
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}

function installDirName(spec: TrustedPackageSpec): string {
  return `${spec.package.replace(/\//g, "__")}@${spec.version}`;
}

/**
 * Download (unless the verified cache already matches), verify, extract and
 * `require` a pinned package. The network result never reaches the guest:
 * only the `require`d host exports do, via {@link nativePackageCapability}.
 */
export async function installTrustedPackage(
  policy: TrustedModulesPolicy,
  spec: TrustedPackageSpec,
  transport: FetchTransport = fetch,
): Promise<LoadedTrustedPackage> {
  assertTrustedSpec(policy, spec);
  const dir = ensureModulesDir(policy.dir ?? DEFAULT_MODULES_DIR);
  const dest = join(dir, installDirName(spec));
  const markerPath = join(dest, ".verified.json");
  const marker = JSON.stringify({ package: spec.package, version: spec.version, integrity: spec.integrity ?? null });

  let cached = false;
  if (existsSync(markerPath)) {
    try {
      cached = readFileSync(markerPath, "utf8") === marker;
    } catch {
      cached = false;
    }
  }

  if (!cached) {
    const registries =
      policy.registries && policy.registries.length > 0 ? policy.registries : [DEFAULT_REGISTRY];
    let data: Uint8Array | null = null;
    let lastError = "";
    for (const registry of registries) {
      const url = packageTarballUrl(registry, spec.package, spec.version);
      try {
        const response = await transport(url);
        if (!response.ok) {
          lastError = `${url}: HTTP ${response.status}`;
          continue;
        }
        const bytes = new Uint8Array(await response.arrayBuffer());
        if (bytes.length > MAX_TARBALL_BYTES) {
          throw new PluginLoadError("tarball exceeds the size limit: refused");
        }
        data = bytes;
        break;
      } catch (error) {
        lastError = error instanceof Error ? error.message : String(error);
      }
    }
    if (!data) throw new PluginLoadError(`download failed for ${spec.package}@${spec.version}: ${lastError}`);
    if (spec.integrity) verifyIntegrity(data, spec.integrity);
    rmSync(dest, { recursive: true, force: true });
    mkdirSync(dest, { recursive: true });
    extractTarball(await gunzip(data), dest);
    writeFileSync(markerPath, marker);
  }

  const manifestPath = join(dest, "package", "package.json");
  if (!existsSync(manifestPath)) {
    throw new PluginLoadError(`package "${spec.package}" has no package/package.json`);
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as { main?: string };
  const entry = join(dest, "package", manifest.main ?? "index.js");
  if (!existsSync(entry)) {
    throw new PluginLoadError(`package "${spec.package}" entry not found: ${manifest.main ?? "index.js"}`);
  }
  const require = createRequire(entry);
  const loaded = require(entry) as Record<string, unknown>;
  return { name: spec.package, version: spec.version, dir: dest, entry, exports: loaded ?? {} };
}

// ── host → registry ──────────────────────────────────────────────────

export interface NativePackageCapabilityOptions {
  /** Registry name plugins request, e.g. `"greet"`. */
  exposeAs: string;
  /** Result of {@link installTrustedPackage}. */
  loaded: { exports: Record<string, unknown> };
  /** Closed allowlist over the loaded exports (see `native-bridge.ts`). */
  definition: NativeModuleDefinition;
}

/**
 * Turn a verified package into a capability definition and register it, so
 * plugins request it by `exposeAs` and the host grants it via
 * `policy.capabilities`. The install returns its own teardown, like every
 * other capability. Throws when the name is taken.
 */
export function nativePackageCapability(options: NativePackageCapabilityOptions): CapabilityDefinition {
  const definition: CapabilityDefinition = {
    name: options.exposeAs,
    install: ({ vm, checker }) => {
      const installed = installNativeModule(vm, options.definition, {
        target: options.loaded.exports,
        checker,
      });
      return () => installed.uninstall(vm);
    },
  };
  defineCapability(definition);
  return definition;
}
