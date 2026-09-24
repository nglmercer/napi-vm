/**
 * The `node:path` compatibility facade: path manipulation without I/O.
 *
 * Guest path behavior follows the configured host platform, matching Node's
 * `node:path` module on that platform.
 *
 * Nothing here touches the filesystem, so the helpers need no filesystem
 * grant. The module is registered only when the manifest asks for it.
 */

import type { Vm } from "../../index";
import { posixPath, type HostPath } from "../platform";
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
  install: ({ vm, platform }) => installPathCapability(vm, platform.path),
};

defineCapability(PATH_CAPABILITY);

const PATH_GLOBALS = [
  "__cap_path_join",
  "__cap_path_normalize",
  "__cap_path_dirname",
  "__cap_path_basename",
  "__cap_path_extname",
  "__cap_path_resolve",
  "__cap_path_relative",
  "__cap_path_isAbsolute",
  "__cap_path_sep",
] as const;

export const PATH_MODULE_NAME = "node:path";

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

export function resolve(...parts) {
  return __cap_path_resolve(...parts);
}

export function relative(from, to) {
  return __cap_path_relative(from, to);
}

export function isAbsolute(path) {
  return __cap_path_isAbsolute(path);
}

export const sep = __cap_path_sep();
`;

/** Expose the configured host path helpers and register `node:path`. */
export function installPathCapability(
  vm: Vm,
  hostPath: HostPath = posixPath,
): CapabilityTeardown {
  vm.exposeFunction("__cap_path_join", (...parts: unknown[]) =>
    hostPath.join(...parts.map((part) => String(part))),
  );
  vm.exposeFunction("__cap_path_normalize", (requestedPath: unknown) =>
    hostPath.normalize(String(requestedPath)),
  );
  vm.exposeFunction("__cap_path_dirname", (requestedPath: unknown) =>
    hostPath.dirname(String(requestedPath)),
  );
  vm.exposeFunction("__cap_path_basename", (requestedPath: unknown, ext: unknown) =>
    ext === undefined || ext === null
      ? hostPath.basename(String(requestedPath))
      : hostPath.basename(String(requestedPath), String(ext)),
  );
  vm.exposeFunction("__cap_path_extname", (requestedPath: unknown) =>
    hostPath.extname(String(requestedPath)),
  );
  vm.exposeFunction("__cap_path_resolve", (...parts: unknown[]) =>
    hostPath.resolve(...parts.map((part) => String(part))),
  );
  vm.exposeFunction("__cap_path_relative", (from: unknown, to: unknown) =>
    hostPath.relative(String(from), String(to)),
  );
  vm.exposeFunction("__cap_path_isAbsolute", (value: unknown) =>
    hostPath.isAbsolute(String(value)),
  );
  vm.exposeFunction("__cap_path_sep", () => hostPath.sep);

  vm.registerModule(PATH_MODULE_NAME, PATH_MODULE_SOURCE);
  return () => unbindCapabilityModule(vm, PATH_MODULE_NAME, PATH_GLOBALS);
}
