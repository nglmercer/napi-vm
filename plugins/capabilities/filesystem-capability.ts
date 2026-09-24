/**
 * The `node:fs` compatibility facade: a deliberately small, fully-checked
 * subset of Node's synchronous filesystem API.
 *
 * The guest sees familiar `node:fs` names. Every path resolves and authorizes
 * itself — registering the module grants nothing on its own.
 */

import type { Vm } from "../../index";

import { PluginManifestError } from "../core/errors";
import type { HostFileSystem } from "../platform";
import { compilePattern, type PathRule } from "../fs/path-rules";
import type { FsPermissionChecker } from "../fs/checker";
import {
  defineCapability,
  unbindCapabilityModule,
  type CapabilityTeardown,
} from "./capability-registry";
export type FsPermission = boolean | "*" | string | string[];

function validateFsPermission(value: unknown, field: string): FsPermission | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "boolean" || typeof value === "string") return value;
  if (Array.isArray(value)) {
    for (const entry of value) {
      if (typeof entry !== "string") {
        throw new PluginManifestError(`${field} array entries must be strings`);
      }
    }
    return value as string[];
  }
  throw new PluginManifestError(`${field} must be boolean, string, or string[]`);
}

defineCapability({
  name: "fs",
  // Infrastructure key: `fs` installs no guest module through the loop — the
  // host builds the shared checker from its compiled form up front. The `fs`
  // capability itself is substrate, always present.
  validate(value, field) {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new PluginManifestError(`${field} must be an object`);
    }
    const fs = value as Record<string, unknown>;
    for (const key of Object.keys(fs)) {
      if (key !== "read" && key !== "write") {
        throw new PluginManifestError(`${field}.${key} is not a permission`);
      }
    }
    const out: { read?: FsPermission; write?: FsPermission } = {};
    const read = validateFsPermission(fs.read, `${field}.read`);
    const write = validateFsPermission(fs.write, `${field}.write`);
    if (read !== undefined) out.read = read;
    if (write !== undefined) out.write = write;
    return out;
  },
  resolve: () => [],
  compile: (request) => {
    const fs = request as { read?: FsPermission; write?: FsPermission };
    return {
      read: compileFsPermission(fs.read, "permissions.fs.read"),
      write: compileFsPermission(fs.write, "permissions.fs.write"),
    };
  },
});

/**
 * Normalize one manifest permission value into rules.
 *
 * `false` / `undefined` → no rules (denied); `true` and `"*"` both become the
 * canonical `{ kind: "all" }`.
 */
export function compileFsPermission(
  permission: FsPermission | undefined,
  field: string,
): PathRule[] {
  if (permission === undefined || permission === false) return [];
  if (permission === true) return [{ kind: "all", pattern: "*" }];
  if (typeof permission === "string") return [compilePattern(permission, field)];
  if (Array.isArray(permission)) {
    if (permission.length === 0) return [];
    return permission.map((pattern) => {
      if (typeof pattern !== "string") {
        throw new PluginManifestError(`${field} array entries must be strings`);
      }
      return compilePattern(pattern, field);
    });
  }
  throw new PluginManifestError(`${field} must be boolean, string, or string[]`);
}

/** Host globals backing the checked `node:fs` facade. Names are convention, never security. */
const FS_GLOBALS = [
  "__cap_fs_readText",
  "__cap_fs_writeText",
  "__cap_fs_exists",
] as const;

export const FS_MODULE_NAME = "node:fs";

const FS_MODULE_SOURCE = `
export function readFileSync(path, encoding) {
  if (encoding !== "utf8" && encoding !== "utf-8") {
    throw new TypeError("the sandboxed node:fs facade supports UTF-8 text reads only");
  }
  return __cap_fs_readText(path);
}

export function writeFileSync(path, contents) {
  __cap_fs_writeText(path, contents);
}

export function existsSync(path) {
  return __cap_fs_exists(path);
}
`;

export interface FsCapabilityOptions {
  checker: FsPermissionChecker;
  fs: HostFileSystem;
}

/**
 * Expose the checked filesystem functions and register the `node:fs` facade.
 * Returns its own teardown — the host runs it on unload, no separate
 * `uninstallFsCapability` to remember.
 */
export function installFsCapability(vm: Vm, options: FsCapabilityOptions): CapabilityTeardown {
  const { checker, fs } = options;

  vm.exposeFunction("__cap_fs_readText", (requestedPath: unknown) => {
    const { native } = checker.resolve(requestedPath, "read");
    return fs.readText(native);
  });

  vm.exposeFunction(
    "__cap_fs_writeText",
    (requestedPath: unknown, contents: unknown) => {
      if (typeof contents !== "string") {
        throw new TypeError("writeText(path, contents): contents must be a string");
      }
      const { native } = checker.resolve(requestedPath, "write");
      fs.writeText(native, contents);
    },
  );

  vm.exposeFunction("__cap_fs_exists", (requestedPath: unknown) => {
    const { native } = checker.resolve(requestedPath, "read");
    return fs.exists(native);
  });

  vm.registerModule(FS_MODULE_NAME, FS_MODULE_SOURCE);
  return () => unbindCapabilityModule(vm, FS_MODULE_NAME, FS_GLOBALS);
}
