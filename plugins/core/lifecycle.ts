/**
 * Guest-side lifecycle glue.
 *
 * The entry module is registered as `plugin:<name>` and its default export —
 * an object *or* a class — is normalized to a single instance behind three
 * wrapper functions the host can reach with `vm.callFunction`.
 */

import type { Vm } from "../../index";

export const PLUGIN_MODULE_PREFIX = "plugin:";

export const LIFECYCLE_GLOBALS = [
  "__plugin_onLoad",
  "__plugin_onUnload",
  "__plugin_onReload",
  "__plugin_describe",
] as const;

export type UnloadReason = "unload" | "reload";

export interface PluginContext {
  name: string;
  version: string;
}

export interface UnloadContext extends PluginContext {
  reason: UnloadReason;
}

export function pluginModuleName(name: string): string {
  return `${PLUGIN_MODULE_PREFIX}${name}`;
}

/**
 * Source evaluated once per VM, after the capabilities are installed.
 *
 * `moduleName` is interpolated through `JSON.stringify`; manifest validation
 * already restricts plugin names to `[A-Za-z0-9._-]`.
 */
export function bootstrapSource(moduleName: string): string {
  return `
import __Plugin from ${JSON.stringify(moduleName)};

const __pluginInstance =
  typeof __Plugin === "function" ? new __Plugin() : __Plugin;

function __plugin_describe() {
  return {
    hasInstance: __pluginInstance !== undefined && __pluginInstance !== null,
    isClass: typeof __Plugin === "function"
  };
}

function __plugin_onLoad(context) {
  if (__pluginInstance && typeof __pluginInstance.onLoad === "function") {
    return __pluginInstance.onLoad(context);
  }
}

function __plugin_onUnload(context) {
  if (__pluginInstance && typeof __pluginInstance.onUnload === "function") {
    return __pluginInstance.onUnload(context);
  }
}

function __plugin_onReload(context, previousState) {
  if (__pluginInstance && typeof __pluginInstance.onReload === "function") {
    return __pluginInstance.onReload(context, previousState);
  }
  return __plugin_onLoad(context);
}

undefined;
`;
}

export interface PluginShape {
  hasInstance: boolean;
  isClass: boolean;
}

/** Ask the guest what the default export turned out to be. */
export function describePlugin(vm: Vm): PluginShape {
  const described = vm.callFunction("__plugin_describe", []) as Partial<PluginShape> | undefined;
  return {
    hasInstance: described?.hasInstance === true,
    isClass: described?.isClass === true,
  };
}

/** Remove the lifecycle wrappers and the entry module from a VM. */
export function uninstallLifecycle(vm: Vm, moduleName: string): void {
  vm.removeModule(moduleName);
  for (const name of LIFECYCLE_GLOBALS) vm.removeGlobal(name);
}
