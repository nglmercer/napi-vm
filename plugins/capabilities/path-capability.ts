/**
 * The `napi:path` capability: pure POSIX path manipulation.
 *
 * Guest paths are POSIX on every host, so a plugin built on Linux behaves
 * identically on Windows. The host converts guest paths to native paths when
 * it actually performs I/O (see `permissions.ts`).
 *
 * Nothing here touches the filesystem, so nothing here needs a permission
 * check — but the module is only registered when the manifest asks for it.
 */

import * as nodePath from "node:path";

import type { Vm } from "../../index";
import {
  unbindCapabilityModule,
  type CapabilityTeardown,
} from "./capability-registry";

const PATH_GLOBALS = [
  "__cap_path_join",
  "__cap_path_normalize",
  "__cap_path_dirname",
  "__cap_path_basename",
  "__cap_path_extname",
] as const;

export const PATH_MODULE_NAME = "napi:path";

const PATH_MODULE_SOURCE = `
export function join(...parts) {
  return __cap_path_join(...parts);
}

export function normalize(path) {
  return __cap_path_normalize(path);
}

export function dirname(path) {
  return __cap_path_dirname(path);
}

export function basename(path, ext) {
  return __cap_path_basename(path, ext);
}

export function extname(path) {
  return __cap_path_extname(path);
}
`;

const posix = nodePath.posix;

/** Expose the POSIX path helpers and register `napi:path`; returns its teardown. */
export function installPathCapability(vm: Vm): CapabilityTeardown {
  vm.exposeFunction("__cap_path_join", (...parts: unknown[]) =>
    posix.join(...parts.map((part) => String(part))),
  );
  vm.exposeFunction("__cap_path_normalize", (path: unknown) => posix.normalize(String(path)));
  vm.exposeFunction("__cap_path_dirname", (path: unknown) => posix.dirname(String(path)));
  vm.exposeFunction("__cap_path_basename", (path: unknown, ext: unknown) =>
    ext === undefined || ext === null
      ? posix.basename(String(path))
      : posix.basename(String(path), String(ext)),
  );
  vm.exposeFunction("__cap_path_extname", (path: unknown) => posix.extname(String(path)));

  vm.registerModule(PATH_MODULE_NAME, PATH_MODULE_SOURCE);
  return () => unbindCapabilityModule(vm, PATH_MODULE_NAME, PATH_GLOBALS);
}
