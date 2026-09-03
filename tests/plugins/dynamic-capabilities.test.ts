import { afterEach, test, expect } from "bun:test";

import { createHash } from "node:crypto";
import { mkdtempSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gzipSync } from "node:zlib";

import type { PlaybackState } from "miniaudio_node";

import { Vm } from "../../index.js";
import {
  applyCapabilityOptions,
  assertTrustedSpec,
  AUDIO_CAPABILITY,
  compilePermissions,
  compileFsPolicy,
  createNodeFileSystem,
  defaultPolicy,
  type CompiledFsPermissions,
  defineCapability,
  ensureModulesDir,
  extractTarball,
  FsPermissionChecker,
  getCapability,
  hasCapability,
  installTrustedPackage,
  listCapabilities,
  nativePackageCapability,
  packageTarballUrl,
  unbindCapabilityModule,
  unregisterCapability,
  validateManifest,
  verifyIntegrity,
  type AudioPlayerLike,
  type TrustedModulesPolicy,
} from "../../plugins";
import { cleanup, makeHost, makePlugin, manifestWith } from "./helpers";

// ---------------------------------------------------------------------------
// Dynamic capabilities: names resolve against a host-side registry, not a
// hardcoded manifest enum. Request ∩ policy ∩ runtime switch = installed.
// ---------------------------------------------------------------------------

const definedHere: string[] = [];
function uniqueCapability(prefix: string): string {
  return `${prefix}-${definedHere.length}`;
}

afterEach(() => {
  cleanup();
  while (definedHere.length > 0) unregisterCapability(definedHere.pop()!);
});

function defineEchoCapability(name: string): void {
  defineCapability({
    name,
    install: ({ vm }) => {
      const globals = vm.registerHostModule(`napi:${name}`, {
        echo: (value: unknown) => `echo:${String(value)}`,
      });
      return () => unbindCapabilityModule(vm, `napi:${name}`, globals);
    },
  });
  definedHere.push(name);
}

test("registry defines, lists, and rejects duplicates and bad names", () => {
  const name = uniqueCapability("reg");
  expect(hasCapability(name)).toBe(false);
  defineEchoCapability(name);
  expect(hasCapability(name)).toBe(true);
  expect(getCapability(name)?.name).toBe(name);
  expect(listCapabilities()).toContain(name);
  expect(() => defineEchoCapability(name)).toThrow(/already defined/);
  expect(() =>
    defineCapability({ name: "../evil", install: () => () => {} }),
  ).toThrow(/must match/);
});

test("unregistering removes the capability", () => {
  const name = uniqueCapability("unreg");
  defineEchoCapability(name);
  expect(unregisterCapability(name)).toBe(true);
  expect(hasCapability(name)).toBe(false);
  expect(unregisterCapability(name)).toBe(false);
  definedHere.pop();
});

test("manifest validates the capabilities shape, not the names", () => {
  const ok = validateManifest(
    manifestWith({ capabilities: { audio: true, greet: { voice: "alto" } } }),
  );
  expect(ok.permissions?.capabilities).toEqual({ audio: true, greet: { voice: "alto" } });

  for (const bad of ["audio", 1, ["audio"], null]) {
    expect(() => validateManifest(manifestWith({ capabilities: bad }))).toThrow(
      /must be an object/,
    );
  }
  expect(() =>
    validateManifest(manifestWith({ capabilities: { "../evil": true } })),
  ).toThrow(/invalid capability name/);
  expect(() =>
    validateManifest(manifestWith({ capabilities: { greet: ["x"] } })),
  ).toThrow(/boolean or an options object/);
});

test("requested and granted capability is usable from guest code", () => {
  const name = uniqueCapability("echo");
  defineEchoCapability(name);
  const dir = makePlugin({
    manifest: manifestWith({ capabilities: { [name]: true } }),
    entry: `import { echo } from "napi:${name}";
export default { onLoad() { return echo("hi"); } };`,
  });
  const host = makeHost({ policy: { ...defaultPolicy(), capabilities: { [name]: true } } });
  const plugin = host.load(dir);
  expect(plugin.loadResult).toBe("echo:hi");
  expect(plugin.capabilities).toEqual([name]);
});

test("unknown capability names fail the load, not the parse", () => {
  const dir = makePlugin({
    manifest: manifestWith({ capabilities: { "missing-xyz": true } }),
    entry: `export default { onLoad() { return "ok"; } };`,
  });
  const host = makeHost({
    policy: { ...defaultPolicy(), capabilities: { "missing-xyz": true } },
  });
  expect(() => host.load(dir)).toThrow(/unknown capability "missing-xyz"/);
});

test("requested but ungranted capability stays absent", () => {
  const name = uniqueCapability("denied");
  defineEchoCapability(name);
  const dir = makePlugin({
    manifest: manifestWith({ capabilities: { [name]: true } }),
    entry: `export default { onLoad() { return "ok"; } };`,
  });
  const host = makeHost();
  const plugin = host.load(dir);
  expect(plugin.capabilities).toEqual([]);
  expect(() => plugin.vm.run(`import { echo } from "napi:${name}"; echo("x");`)).toThrow();
});

test("explicit false opts out without error", () => {
  const name = uniqueCapability("optout");
  defineEchoCapability(name);
  const dir = makePlugin({
    manifest: manifestWith({ capabilities: { [name]: false } }),
    entry: `export default { onLoad() { return "ok"; } };`,
  });
  const host = makeHost({ policy: { ...defaultPolicy(), capabilities: { [name]: true } } });
  const plugin = host.load(dir);
  expect(plugin.loadResult).toBe("ok");
  expect(plugin.capabilities).toEqual([]);
});

test("setCapabilityEnabled disables at runtime; unknown names throw", () => {
  const name = uniqueCapability("toggle");
  defineEchoCapability(name);
  const entry = `export default { onLoad() { return "ok"; } };`;
  const policy = { ...defaultPolicy(), capabilities: { [name]: true } };

  expect(() => makeHost().setCapabilityEnabled("missing-xyz", false)).toThrow(
    /unknown capability/,
  );

  const host = makeHost({ policy });
  const dir = makePlugin({ manifest: manifestWith({ capabilities: { [name]: true } }), entry });
  expect(host.isCapabilityEnabled(name)).toBe(true);

  host.load(dir);
  host.unload("test-plugin");
  host.setCapabilityEnabled(name, false);
  expect(host.isCapabilityEnabled(name)).toBe(false);
  const plugin = host.load(dir);
  expect(plugin.capabilities).toEqual([]);
  expect(() => plugin.vm.run(`import { echo } from "napi:${name}"; echo("x");`)).toThrow();

  host.unload("test-plugin");
  host.setCapabilityEnabled(name, true);
  expect(host.load(dir).capabilities).toEqual([name]);
});

test("unload revokes the dynamic module", () => {
  const name = uniqueCapability("revoke");
  defineEchoCapability(name);
  const dir = makePlugin({
    manifest: manifestWith({ capabilities: { [name]: true } }),
    entry: `export default { onLoad() { return "ok"; } };`,
  });
  const host = makeHost({ policy: { ...defaultPolicy(), capabilities: { [name]: true } } });
  const plugin = host.load(dir);
  expect(plugin.vm.hasModule(`napi:${name}`)).toBe(true);
  host.unload("test-plugin");
  expect(plugin.vm.hasModule(`napi:${name}`)).toBe(false);
});

// ---------------------------------------------------------------------------
// Trusted packages: pinned version + integrity + allowlist, verified before
// any network or filesystem write. The guest never sees this layer.
// ---------------------------------------------------------------------------

const trustedPolicy: TrustedModulesPolicy = {
  dir: join(tmpdir(), "napi-vm-trusted-test"),
  registries: ["https://registry.example.com"],
  allow: ["fake-pkg", "@myorg/*"],
};

test("assertTrustedSpec fails closed", () => {
  const good = { package: "fake-pkg", version: "1.2.3", integrity: "sha512-aaa=" };
  expect(() => assertTrustedSpec(trustedPolicy, good)).not.toThrow();
  expect(() =>
    assertTrustedSpec(trustedPolicy, { ...good, package: "../evil" }),
  ).toThrow(/untrusted package name/);
  for (const version of ["^1.2.3", "latest", "1.2", ""]) {
    expect(() => assertTrustedSpec(trustedPolicy, { ...good, version })).toThrow(
      /exact version/,
    );
  }
  expect(() => assertTrustedSpec(trustedPolicy, { ...good, package: "other-pkg" })).toThrow(
    /allowlist/,
  );
  expect(() => assertTrustedSpec(trustedPolicy, { package: "fake-pkg", version: "1.2.3" })).toThrow(
    /integrity/,
  );
  expect(() => assertTrustedSpec(trustedPolicy, { ...good, integrity: "md5-aaa" })).toThrow(
    /malformed integrity/,
  );
  expect(() =>
    assertTrustedSpec(trustedPolicy, {
      package: "@myorg/voice",
      version: "0.0.1",
      integrity: "sha512-aaa=",
    }),
  ).not.toThrow();
});

test("packageTarballUrl builds scoped URLs and refuses plain http", () => {
  expect(packageTarballUrl("https://registry.npmjs.org/", "miniaudio_node", "1.6.3")).toBe(
    "https://registry.npmjs.org/miniaudio_node/-/miniaudio_node-1.6.3.tgz",
  );
  expect(packageTarballUrl("https://r.example", "@myorg/voice", "0.0.1")).toBe(
    "https://r.example/@myorg/voice/-/voice-0.0.1.tgz",
  );
  expect(() => packageTarballUrl("http://evil.example", "x", "1.0.0")).toThrow(/https/);
  expect(packageTarballUrl("http://127.0.0.1:4873", "x", "1.0.0")).toContain("x/-/x-1.0.0.tgz");
});

test("verifyIntegrity accepts the pinned hash and refuses the rest", () => {
  const data = new TextEncoder().encode("hello modules");
  const digest = createHash("sha512").update(data).digest("base64");
  expect(() => verifyIntegrity(data, `sha512-${digest}`)).not.toThrow();
  expect(() => verifyIntegrity(data, `sha512-${digest.slice(0, -2)}aa`)).toThrow(/mismatch/);
  expect(() => verifyIntegrity(data, "nope")).toThrow(/sha512-<base64>/);
});

test("extractTarball round-trips files and refuses escapes", () => {
  const root = mkdtempSync(join(tmpdir(), "napi-vm-tar-"));
  try {
    const files = extractTarball(
      makeTar({ "package/index.cjs": "module.exports = 1;", "package/lib/x.js": "2" }),
      root,
    );
    expect(files.sort()).toEqual(["package/index.cjs", "package/lib/x.js"]);
    expect(() => extractTarball(makeTar({ "../evil.js": "x" }), root)).toThrow(/escapes/);
    expect(() => extractTarball(makeTar({ "/abs.js": "x" }), root)).toThrow(/escapes/);
    expect(() => extractTarball(new Uint8Array(100), root)).not.toThrow();
    expect(() => extractTarball(new Uint8Array([1, 2, 3]), root)).not.toThrow();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("ensureModulesDir creates the folder", () => {
  const dir = join(tmpdir(), `napi-vm-mods-${Date.now()}`);
  try {
    expect(ensureModulesDir(dir)).toBe(dir);
    expect(ensureModulesDir(dir)).toBe(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

// --- minimal ustar writer for fixtures (headers this reader understands) ---

function tarEntry(name: string, text: string): Uint8Array {
  const data = new TextEncoder().encode(text);
  const header = new Uint8Array(512);
  const writeStr = (value: string, offset: number, length: number) => {
    header.set(new TextEncoder().encode(value).subarray(0, length), offset);
  };
  const writeOct = (value: number, offset: number, length: number) => {
    writeStr(value.toString(8).padStart(length - 1, "0"), offset, length - 1);
  };
  writeStr(name, 0, 100);
  writeOct(0o644, 100, 8);
  writeOct(0, 108, 8);
  writeOct(0, 116, 8);
  writeOct(data.length, 124, 12);
  writeOct(0, 136, 12);
  header[156] = "0".charCodeAt(0);
  writeStr("ustar", 257, 6);
  writeStr("00", 263, 2);
  for (let i = 148; i < 156; i++) header[i] = 32;
  let sum = 0;
  for (const byte of header) sum += byte;
  writeOct(sum, 148, 8);
  const body = new Uint8Array(Math.ceil(data.length / 512) * 512);
  body.set(data);
  const out = new Uint8Array(512 + body.length);
  out.set(header);
  out.set(body, 512);
  return out;
}

function makeTar(files: Record<string, string>): Uint8Array {
  const parts = Object.entries(files).map(([name, text]) => tarEntry(name, text));
  const total = parts.reduce((sum, part) => sum + part.length, 0) + 1024;
  const out = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

// --- end-to-end: serve a tarball, verify, require, expose, call from guest ---

/**
 * Stub transport: serves the tarball without touching the network (hermetic
 * under sandboxes/proxies) while asserting the exact tarball URL requested.
 */
function tarballTransport(
  tarball: Uint8Array,
  seen: string[],
): (url: string) => Promise<Response> {
  return async (url: string) => {
    seen.push(url);
    return new Response(Buffer.from(tarball));
  };
}

const FAKE_FILES = {
  "package/package.json": JSON.stringify({ name: "fake-pkg", version: "9.9.9", main: "index.cjs" }),
  "package/index.cjs": `module.exports = { hello: (name) => "hi " + name };`,
};

function fakeIntegrity(tarball: Uint8Array): string {
  return `sha512-${createHash("sha512").update(tarball).digest("base64")}`;
}

test("download → verify → require → expose → guest call", async () => {
  const tarball = gzipSync(makeTar(FAKE_FILES));
  const seen: string[] = [];
  const dir = mkdtempSync(join(tmpdir(), "napi-vm-e2e-"));
  const exposeAs = uniqueCapability("greet");
  try {
    const policy: TrustedModulesPolicy = {
      dir,
      registries: ["https://registry.example.com"],
      allow: ["fake-pkg"],
    };
    const loaded = await installTrustedPackage(
      policy,
      { package: "fake-pkg", version: "9.9.9", integrity: fakeIntegrity(tarball) },
      tarballTransport(tarball, seen),
    );
    expect(seen).toEqual([
      "https://registry.example.com/fake-pkg/-/fake-pkg-9.9.9.tgz",
    ]);
    expect(typeof (loaded.exports as { hello: unknown }).hello).toBe("function");

    // Verified cache: a dead transport still loads from disk.
    const cached = await installTrustedPackage(
      policy,
      { package: "fake-pkg", version: "9.9.9", integrity: fakeIntegrity(tarball) },
      () => {
        throw new Error("network must not be touched on cache hit");
      },
    );
    expect(cached.entry).toBe(loaded.entry);

    nativePackageCapability({
      exposeAs,
      loaded,
      definition: { moduleName: `napi:${exposeAs}`, methods: { hello: {} } },
    });
    definedHere.push(exposeAs);

    const pluginDir = makePlugin({
      manifest: manifestWith({ capabilities: { [exposeAs]: true } }),
      entry: `import { hello } from "napi:${exposeAs}";
export default { onLoad() { return hello("world"); } };`,
    });
    const host = makeHost({
      policy: { ...defaultPolicy(), capabilities: { [exposeAs]: true } },
    });
    expect(host.load(pluginDir).loadResult).toBe("hi world");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("tampered tarball is refused before require", async () => {
  const tarball = gzipSync(makeTar(FAKE_FILES));
  const dir = mkdtempSync(join(tmpdir(), "napi-vm-e2e-bad-"));
  try {
    await expect(
      installTrustedPackage(
        { dir, registries: ["https://registry.example.com"], allow: ["fake-pkg"] },
        { package: "fake-pkg", version: "9.9.9", integrity: "sha512-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==" },
        tarballTransport(tarball, []),
      ),
    ).rejects.toThrow(/mismatch/);
    expect(existsMarker(dir)).toBe(false);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("registry fallback tries the next mirror after an HTTP error", async () => {
  const tarball = gzipSync(makeTar(FAKE_FILES));
  const seen: string[] = [];
  const failing = async (url: string): Promise<Response> => {
    seen.push(url);
    return new Response("nope", { status: 404 });
  };
  const dir = mkdtempSync(join(tmpdir(), "napi-vm-e2e-fallback-"));
  try {
    const loaded = await installTrustedPackage(
      {
        dir,
        registries: ["https://bad.example.com", "https://good.example.com"],
        allow: ["fake-pkg"],
      },
      { package: "fake-pkg", version: "9.9.9", integrity: fakeIntegrity(tarball) },
      (async (url: string) =>
        url.includes("bad.example.com") ? failing(url) : tarballTransport(tarball, seen)(url)),
    );
    expect(loaded.version).toBe("9.9.9");
    expect(seen).toEqual([
      "https://bad.example.com/fake-pkg/-/fake-pkg-9.9.9.tgz",
      "https://good.example.com/fake-pkg/-/fake-pkg-9.9.9.tgz",
    ]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

function existsMarker(dir: string): boolean {
  try {
    return readdirSync(dir).length > 0;
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// `napi:audio` through the registry: paths checked, ranges enforced, no
// native dependency in tests (injected fake player).
// ---------------------------------------------------------------------------

function makeFakePlayer() {
  const calls: Array<[string, unknown[]]> = [];
  const state = { volume: 1, file: null as string | null };
  const player: AudioPlayerLike = {
    getDevices: () => [],
    loadFile: (filePath: string) => {
      calls.push(["loadFile", [filePath]]);
      state.file = filePath;
    },
    loadBuffer: (audioData: number[]) => {
      calls.push(["loadBuffer", [audioData.length]]);
    },
    loadBase64: (base64Data: string) => {
      calls.push(["loadBase64", [base64Data.length]]);
    },
    play: () => {
      calls.push(["play", []]);
    },
    pause: () => {
      calls.push(["pause", []]);
    },
    stop: () => {
      calls.push(["stop", []]);
    },
    setVolume: (volume: number) => {
      calls.push(["setVolume", [volume]]);
      state.volume = volume;
    },
    getVolume: () => state.volume,
    isPlaying: () => false,
    // The runtime enum export is empty headless; the type still checks.
    getState: () => "Stopped" as PlaybackState,
    getDuration: () => 180,
    getCurrentTime: () => 0,
    getCurrentFile: () => state.file,
    seekTo: (position: number) => {
      calls.push(["seekTo", [position]]);
    },
  };
  return { calls, player };
}

function makeAudioRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "napi-vm-audio-"));
  mkdirSync(join(root, "audio"), { recursive: true });
  writeFileSync(join(root, "audio", "track.mp3"), "fake-audio-bytes");
  return root;
}

function makeAudioVm(root: string): {
  vm: Vm;
  calls: Array<[string, unknown[]]>;
  teardown: () => void;
} {
  const { calls, player } = makeFakePlayer();
  const manifest = validateManifest(
    manifestWith({ fs: { read: ["./audio/**"] }, capabilities: { audio: true } }),
  );
  const permissions = compilePermissions(manifest);
  const checker = new FsPermissionChecker(
    root,
    // Sound cast: the `fs` binding compiled these rules at load.
    permissions.fs as CompiledFsPermissions,
    compileFsPolicy(defaultPolicy().fs),
    createNodeFileSystem(),
  );
  const vm = new Vm();
  const teardown = AUDIO_CAPABILITY.install({
    vm,
    manifest,
    permissions,
    checker,
    options: true,
    grant: { createPlayer: () => player },
  });
  return { vm, calls, teardown };
}

test("audio loadFile receives the canonical path, not the guest string", () => {
  const root = makeAudioRoot();
  try {
    const { vm, calls } = makeAudioVm(root);
    vm.run(`import { loadFile, getCurrentFile } from "napi:audio"; loadFile("./audio/track.mp3");`);
    expect(calls).toEqual([["loadFile", [join(root, "audio", "track.mp3")]]]);
    expect(vm.run(`import { getCurrentFile } from "napi:audio"; getCurrentFile();`)).toBe(
      join(root, "audio", "track.mp3"),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("audio refuses paths outside the grant and bad ranges", () => {
  const root = makeAudioRoot();
  try {
    const { vm, calls } = makeAudioVm(root);
    expect(() =>
      vm.run(`import { loadFile } from "napi:audio"; loadFile("./other/secret.mp3");`),
    ).toThrow(/PermissionDenied/);
    expect(() =>
      vm.run(`import { loadFile } from "napi:audio"; loadFile("../outside.mp3");`),
    ).toThrow(/PermissionDenied|escapes/);
    expect(() => vm.run(`import { setVolume } from "napi:audio"; setVolume(2);`)).toThrow(
      /0\.\.1/,
    );
    expect(() => vm.run(`import { seekTo } from "napi:audio"; seekTo(-1);`)).toThrow(
      /non-negative/,
    );
    expect(calls).toEqual([]);
    vm.run(`import { setVolume, getVolume } from "napi:audio"; setVolume(0.5);`);
    expect(vm.run(`import { getVolume } from "napi:audio"; getVolume();`)).toBe("0.5");
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("audio uninstall removes the module", () => {
  const root = makeAudioRoot();
  try {
    const { vm, teardown } = makeAudioVm(root);
    expect(vm.hasModule("napi:audio")).toBe(true);
    teardown();
    expect(vm.hasModule("napi:audio")).toBe(false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("audio options take defaults and refuse unknown keys", () => {
  expect(applyCapabilityOptions("audio", AUDIO_CAPABILITY.schema, true)).toEqual({
    maxAudioBytes: 8 * 1024 * 1024,
  });
  expect(
    applyCapabilityOptions("audio", AUDIO_CAPABILITY.schema, { maxAudioBytes: 1024 }),
  ).toEqual({ maxAudioBytes: 1024 });
  expect(() =>
    applyCapabilityOptions("audio", AUDIO_CAPABILITY.schema, { voice: "alto" }),
  ).toThrow(/unknown option "voice"/);
  expect(() =>
    applyCapabilityOptions("audio", AUDIO_CAPABILITY.schema, { maxAudioBytes: 0 }),
  ).toThrow(/>= 1/);
  expect(() => applyCapabilityOptions("crypto", undefined, { x: 1 })).toThrow(/takes no options/);
  expect(applyCapabilityOptions("crypto", undefined, true)).toEqual({});
});

test("audio through the full host: request ∩ grant installs it", () => {
  const dir = makePlugin({
    manifest: manifestWith({
      fs: { read: ["./audio/**"] },
      path: true,
      capabilities: { audio: true },
    }),
    entry: `import { getDevices, isPlaying } from "napi:audio";
import { exists } from "napi:fs";
import { join } from "napi:path";
export default {
  onLoad() {
    if (!exists(join("./audio", "track.mp3"))) throw new Error("missing track");
    return getDevices().length + ":" + isPlaying();
  },
};`,
    files: { "audio/track.mp3": "fake-audio-bytes" },
  });
  // The real native player answers through the guest bridge (headless: no
  // devices, nothing playing).
  const host = makeHost({
    policy: { ...defaultPolicy(), capabilities: { audio: true } },
  });
  expect(host.load(dir).loadResult).toBe("0:false");
});

/** Minimal WAV writer: 8 kHz mono 16-bit silence. */
function writeWav(path: string, seconds: number): void {
  const sampleRate = 8000;
  const samples = Math.floor(sampleRate * seconds);
  const data = Buffer.alloc(44 + samples * 2);
  data.write("RIFF", 0);
  data.writeUInt32LE(36 + samples * 2, 4);
  data.write("WAVE", 8);
  data.write("fmt ", 12);
  data.writeUInt32LE(16, 16);
  data.writeUInt16LE(1, 20);
  data.writeUInt16LE(1, 22);
  data.writeUInt32LE(sampleRate, 24);
  data.writeUInt32LE(sampleRate * 2, 28);
  data.writeUInt16LE(2, 32);
  data.writeUInt16LE(16, 34);
  data.write("data", 36);
  data.writeUInt32LE(samples * 2, 40);
  writeFileSync(path, data);
}

test("real player decodes a wav through the guest bridge", () => {
  const root = makeAudioRoot();
  writeWav(join(root, "audio", "track.wav"), 1);
  try {
    const manifest = validateManifest(
      manifestWith({ fs: { read: ["./audio/**"] }, capabilities: { audio: true } }),
    );
    const permissions = compilePermissions(manifest);
    const checker = new FsPermissionChecker(
      root,
      // Sound cast: the `fs` binding compiled these rules at load.
      permissions.fs as CompiledFsPermissions,
      compileFsPolicy(defaultPolicy().fs),
      createNodeFileSystem(),
    );
    const vm = new Vm();
    // No factory injected: the default `require("miniaudio_node")` runs.
    const teardown = AUDIO_CAPABILITY.install({
      vm,
      manifest,
      permissions,
      checker,
      options: true,
      grant: true,
    });
    try {
      vm.run(`import { loadFile } from "napi:audio"; loadFile("./audio/track.wav");`);
      expect(
        vm.run(`import { getDuration } from "napi:audio"; Math.abs(getDuration() - 1) < 0.05;`),
      ).toBe("true");
      expect(
        vm.run(`import { getCurrentFile } from "napi:audio"; getCurrentFile().endsWith("track.wav");`),
      ).toBe("true");
      expect(vm.run(`import { isPlaying } from "napi:audio"; isPlaying();`)).toBe("false");
    } finally {
      teardown();
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
