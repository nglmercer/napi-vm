/**
 * The `napi:path` capability: pure POSIX path manipulation.
 *
 * Guest paths are POSIX on every host, so a plugin built on Linux behaves
 * identically on Windows. The host converts guest paths to native paths when
 * it actually performs I/O (see `permissions.ts`).
 *
 * Nothing here touches the filesystem, so nothing here needs a permission
 * check — but the module is only registered when the manifest asks for it.
 * The helpers run on the portable POSIX implementation, never on the host's
 * native `node:path`, so no host import is needed.
 */

import type { Vm } from "../../index";
import { posixPath } from "../platform";
import {
  booleanPermissionValue,
  defineCapability,
  unbindCapabilityModule,
  type CapabilityDefinition,
  type CapabilityTeardown,
} from "./capability-registry";

export const PATH_CAPABILITY: CapabilityDefinition = {
  name: "path",
  validate: booleanPermissionValue,
  // Manifest-only gate: `path: true` installs with no host grant.
  // The grant is ignored on purpose — path helpers cannot reach the host fs.
  allows: (request) => request === true,
  install: ({ vm }) => installPathCapability(vm),
};

defineCapability(PATH_CAPABILITY);

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

/** Expose the POSIX path helpers and register `napi:path`; returns its teardown. */
export function installPathCapability(vm: Vm): CapabilityTeardown {
  vm.exposeFunction("__cap_path_join", (...parts: unknown[]) =>
    posixPath.join(...parts.map((part) => String(part))),
  );
  vm.exposeFunction("__cap_path_normalize", (path: unknown) =>
    posixPath.normalize(String(path)),
  );
  vm.exposeFunction("__cap_path_dirname", (path: unknown) =>
    posixPath.dirname(String(path)),
  );
  vm.exposeFunction("__cap_path_basename", (path: unknown, ext: unknown) =>
    ext === undefined || ext === null
      ? posixPath.basename(String(path))
      : posixPath.basename(String(path), String(ext)),
  );
  vm.exposeFunction("__cap_path_extname", (path: unknown) =>
    posixPath.extname(String(path)),
  );

  vm.registerModule(PATH_MODULE_NAME, PATH_MODULE_SOURCE);
  return () => unbindCapabilityModule(vm, PATH_MODULE_NAME, PATH_GLOBALS);
}
