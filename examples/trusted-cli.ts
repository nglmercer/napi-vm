/**
 * Operator CLI: trusted package install + plugin loading.
 *
 * Downloading code is a trusted-user action, so it lives here — outside any
 * VM — not in a plugin:
 *
 *   # pin, verify and extract into the modules folder
 *   bun examples/trusted-cli.ts install \
 *     --package miniaudio_node --version 1.6.3 \
 *     --integrity sha512-… --allow miniaudio_node
 *
 *   # load a plugin directory (request ∩ policy ∩ kill-switch applies)
 *   bun examples/trusted-cli.ts run ./examples/plugins/example-plugin
 *   bun examples/trusted-cli.ts run ./my-plugin --grant audio
 *
 *   # registry inspection
 *   bun examples/trusted-cli.ts caps
 *
 * Compile to one file (the two `.node` bindings stay next to the binary):
 *   bun build examples/trusted-cli.ts --compile --outfile ./loader
 */

import { join } from "node:path";

import {
  listCapabilities,
  PluginHost,
  type PluginHostPolicy,
} from "../plugins";
import {
  DEFAULT_MODULES_DIR,
  installTrustedPackage,
  nodePlatform,
} from "../plugins/node";

function flag(name: string, fallback?: string): string | undefined {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1) return fallback;
  const value = process.argv[index + 1];
  if (!value || value.startsWith("--")) return fallback;
  return value;
}

function usage(): never {
  console.error(
    "usage:\n" +
      "  trusted-cli.ts install --package <name> --version <x.y.z> --integrity <sha512-…> --allow <name|scope/*> [--dir <dir>] [--registry <url>]\n" +
      "  trusted-cli.ts run <pluginDir> [--grant <a,b>] [--disable <a,b>] [--dir <modulesDir>]\n" +
      "  trusted-cli.ts caps",
  );
  process.exit(2);
}

async function main(): Promise<void> {
  const command = process.argv[2];

  if (command === "caps") {
    // Constructing a host registers the built-ins.
    new PluginHost({ platform: nodePlatform() });
    console.log(listCapabilities().join("\n"));
    return;
  }

  if (command === "install") {
    const pkg = flag("package");
    const version = flag("version");
    const integrity = flag("integrity");
    const allow = flag("allow");
    if (!pkg || !version || !integrity || !allow) usage();
    const loaded = await installTrustedPackage(
      {
        dir: flag("dir", join(process.cwd(), DEFAULT_MODULES_DIR)),
        registries: flag("registry") ? [flag("registry")!] : undefined,
        allow: allow!.split(",").map((entry) => entry.trim()),
      },
      { package: pkg!, version: version!, integrity: integrity! },
    );
    console.log(`verified ${loaded.name}@${loaded.version}`);
    console.log(`dir:   ${loaded.dir}`);
    console.log(`entry: ${loaded.entry}`);
    return;
  }

  if (command === "run") {
    const pluginDir = process.argv[3];
    if (!pluginDir || pluginDir.startsWith("--")) usage();
    const grants = (flag("grant", "") ?? "")
      .split(",")
      .map((entry) => entry.trim())
      .filter((entry) => entry !== "");
    const disables = (flag("disable", "") ?? "")
      .split(",")
      .map((entry) => entry.trim())
      .filter((entry) => entry !== "");
    const capabilities: Record<string, true> = {};
    for (const name of grants) capabilities[name] = true;
    const policy: PluginHostPolicy = {
      capabilities,
    };
    const host = new PluginHost({ policy, platform: nodePlatform() });
    for (const name of disables) host.setCapabilityEnabled(name, false);
    const plugin = host.load(pluginDir);
    console.log(`loaded ${plugin.manifest.name}@${plugin.manifest.version}`);
    console.log(`capabilities: ${plugin.capabilities.join(", ") || "(none)"}`);
    console.log("onLoad returned:", plugin.loadResult);
    return;
  }

  usage();
}

await main();
