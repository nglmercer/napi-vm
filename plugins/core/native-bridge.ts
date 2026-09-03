/**
 * Generic npm → VM bridge: load any host library once, expose an allowlisted
 * subset of its methods as a `napi:*` guest module.
 *
 * This is the sanctioned answer to "load an arbitrary npm / `.node` package
 * inside the VM": the native code NEVER runs inside the interpreter. It is
 * `require`d on the host (normal Node/Bun loader) and only the wrapped
 * functions cross the bridge via `registerHostModule`. String/path arguments
 * are validated before the call, so the guest cannot smuggle an unchecked
 * path into a native `loadFile`.
 *
 * Rules, in order:
 *
 *   1. Closed allowlist — a method not named in `methods` is not exposed,
 *      even if it exists on the target.
 *   2. Fail closed on paths — a method declaring `pathArgs` requires a
 *      `checker`; without one installation throws instead of exposing an
 *      unchecked path sink.
 *   3. Paths resolve through `FsPermissionChecker.resolve(arg, "read")` and
 *      the native side receives the canonical path, never the guest string.
 */

import type { Vm } from "../../index";

import { DEFAULT_MAX_FILE_BYTES } from "../platform";
import { PluginLoadError } from "./errors";
import type { FsPermissionChecker } from "../fs/checker";

/** Per-method exposure policy inside a {@link NativeModuleDefinition}. */
export interface NativeMethodPolicy {
  /**
   * Argument indexes that carry guest paths. Each is resolved through the
   * checker's `read` permission and replaced with the canonical native path
   * before the host function runs.
   */
  pathArgs?: number[];
  /** Extra argument validation; throw to refuse the call. */
  validate?: (args: unknown[]) => void;
}

/** What to expose, and under which guest module name. */
export interface NativeModuleDefinition {
  /** Guest module name, e.g. `"napi:audio"`. */
  moduleName: string;
  /** Allowed methods and their per-method policy. Closed: nothing else leaks. */
  methods: Record<string, NativeMethodPolicy>;
  /** Largest string argument accepted, in bytes. Defaults to 8 MiB. */
  maxStringBytes?: number;
}

/** The host side of an installation: the loaded object plus its checker. */
export interface NativeModuleHost {
  /** The loaded library object (or class instance) to bind methods from. */
  target: Record<string, unknown>;
  /**
   * Permission checker for `pathArgs`. Required when any method declares
   * `pathArgs`; unused otherwise.
   */
  checker?: FsPermissionChecker;
}

const encoder = new TextEncoder();

function byteLength(value: string): number {
  return encoder.encode(value).length;
}

/** An installed native module: globals plus its own teardown. */
export interface InstalledNativeModule {
  readonly moduleName: string;
  readonly globals: readonly string[];
  uninstall(vm: Vm): void;
}

/**
 * Expose `definition.methods` from `host.target` as `definition.moduleName`.
 *
 * Returns a handle carrying the bridge globals and an `uninstall` that
 * removes exactly what this call added — no module-level global list, so two
 * VMs never revoke each other's bridges.
 */
export function installNativeModule(
  vm: Vm,
  definition: NativeModuleDefinition,
  host: NativeModuleHost,
): InstalledNativeModule {
  const names = Object.keys(definition.methods);
  if (names.length === 0) {
    throw new PluginLoadError(
      `native module "${definition.moduleName}" must allow at least one method`,
    );
  }
  const needsChecker = names.some(
    (name) => (definition.methods[name].pathArgs?.length ?? 0) > 0,
  );
  if (needsChecker && !host.checker) {
    throw new PluginLoadError(
      `native module "${definition.moduleName}" declares path arguments but has no permission checker`,
    );
  }
  const maxStringBytes = definition.maxStringBytes ?? DEFAULT_MAX_FILE_BYTES;

  const exports: Record<string, (...args: unknown[]) => unknown> = {};
  for (const name of names) {
    const policy = definition.methods[name];
    const fn = host.target[name];
    if (typeof fn !== "function") {
      throw new PluginLoadError(
        `native module "${definition.moduleName}" has no function "${name}" to expose`,
      );
    }
    const bound = fn as (...args: unknown[]) => unknown;
    const target = host.target;
    exports[name] = (...args: unknown[]) => {
      for (const arg of args) {
        if (typeof arg === "string" && byteLength(arg) > maxStringBytes) {
          throw new RangeError(
            `${definition.moduleName}.${name}: string argument exceeds the ${maxStringBytes} byte limit`,
          );
        }
      }
      const callArgs = args.slice();
      for (const index of policy.pathArgs ?? []) {
        // Fail closed: without a checker this definition could not install,
        // so reaching here with none is unreachable — check anyway.
        if (!host.checker) {
          throw new PluginLoadError(
            `native module "${definition.moduleName}" has no permission checker for paths`,
          );
        }
        callArgs[index] = host.checker.resolve(callArgs[index], "read").native;
      }
      policy.validate?.(callArgs);
      return bound.apply(target, callArgs);
    };
  }
  const globals = vm.registerHostModule(definition.moduleName, exports);
  const moduleName = definition.moduleName;
  return {
    moduleName,
    globals,
    uninstall: (target: Vm) => uninstallNativeModule(target, moduleName, globals),
  };
}

/** Remove everything {@link installNativeModule} added. */
export function uninstallNativeModule(
  vm: Vm,
  moduleName: string,
  globals: readonly string[],
): void {
  vm.removeModule(moduleName);
  for (const name of globals) vm.removeGlobal(name);
}
