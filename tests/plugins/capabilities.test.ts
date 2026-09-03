import { test, expect } from "bun:test";

import { Vm } from "../../index.js";
import {
  checkFetchOrigin,
  compileFetchPermission,
  compilePermissions,
  CRYPTO_CAPABILITY,
  FETCH_CAPABILITY,
  TIMERS_CAPABILITY,
  validateManifest,
  type CapabilityDefinition,
} from "../../plugins";
import { nodePlatform } from "../../plugins/node";
import { manifestWith } from "./helpers";

// ---------------------------------------------------------------------------
// The capability modules beyond `napi:fs` and `napi:path`: `napi:crypto`,
// `napi:timers` and `napi:fetch`. Each is a registry definition installed
// through one interface; each install returns its own teardown, so there is
// no per-capability uninstall function to remember.
// ---------------------------------------------------------------------------

const DEFS: Record<string, CapabilityDefinition> = {
  crypto: CRYPTO_CAPABILITY,
  timers: TIMERS_CAPABILITY,
  fetch: FETCH_CAPABILITY,
};

/** Install a built-in definition directly, returning its teardown. */
function installForTest(
  name: keyof typeof DEFS,
  vm: Vm,
  setup?: { manifestPermissions?: unknown; grant?: unknown },
): () => void {
  const manifest = validateManifest(manifestWith(setup?.manifestPermissions ?? {}));
  return DEFS[name].install!({
    vm,
    manifest,
    permissions: compilePermissions(manifest),
    platform: nodePlatform(),
    options: true,
    grant: setup?.grant ?? true,
  });
}

// --- napi:crypto ------------------------------------------------------------

test("crypto provides UUIDs, random bytes and digests", () => {
  const vm = new Vm();
  installForTest("crypto", vm);
  expect(vm.run("import { randomUUID } from 'napi:crypto'; randomUUID().length;")).toBe("36");
  expect(vm.run("import { randomBytes } from 'napi:crypto'; randomBytes(4).length;")).toBe("4");
  expect(
    vm.run("import { digest } from 'napi:crypto'; digest('sha256', 'abc');"),
  ).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
});

test("getRandomValues fills a typed array", () => {
  const vm = new Vm();
  installForTest("crypto", vm);
  expect(
    vm.run("import { getRandomValues } from 'napi:crypto'; getRandomValues(new Uint8Array(3)).length;"),
  ).toBe("3");
});

test("a huge randomBytes request is refused", () => {
  const vm = new Vm();
  installForTest("crypto", vm);
  expect(() => vm.run("import { randomBytes } from 'napi:crypto'; randomBytes(999999);")).toThrow(
    "limited to",
  );
});

test("an unknown digest algorithm is refused", () => {
  const vm = new Vm();
  installForTest("crypto", vm);
  expect(() => vm.run("import { digest } from 'napi:crypto'; digest('rot13', 'x');")).toThrow(
    "unsupported digest algorithm",
  );
});

test("crypto is absent until installed, and gone after removal", () => {
  const vm = new Vm();
  expect(() => vm.run("import { randomUUID } from 'napi:crypto'; randomUUID();")).toThrow();
  const teardown = installForTest("crypto", vm);
  expect(vm.run("import { randomUUID } from 'napi:crypto'; typeof randomUUID();")).toBe("string");
  teardown();
  expect(() => vm.run("import { randomUUID } from 'napi:crypto'; randomUUID();")).toThrow();
});

// --- napi:timers ------------------------------------------------------------

test("timers exposes the host clock", () => {
  const vm = new Vm();
  installForTest("timers", vm);
  expect(vm.run("import { now } from 'napi:timers'; typeof now();")).toBe("number");
  expect(vm.run("import { now } from 'napi:timers'; now() > 1600000000000;")).toBe("true");
});

test("a coarsened clock hides precision", () => {
  const vm = new Vm();
  installForTest("timers", vm, { grant: { resolutionMs: 1000 } });
  // Rounded down to the second, so the remainder is always zero.
  expect(vm.run("import { now } from 'napi:timers'; now() % 1000;")).toBe("0");
});

test("timers is removable", () => {
  const vm = new Vm();
  const teardown = installForTest("timers", vm);
  teardown();
  expect(() => vm.run("import { now } from 'napi:timers'; now();")).toThrow();
});

test("the VM's own timers stay clock-free without the capability", () => {
  const vm = new Vm();
  // `setTimeout` is always available and always ordered without a clock; the
  // capability is about *observing* time, not scheduling.
  expect(vm.run("typeof setTimeout;")).toBe("function");
  expect(() => vm.run("import { now } from 'napi:timers'; now();")).toThrow();
});

// --- napi:fetch: the permission check ---------------------------------------

function permitted(requested: unknown, policyAllow: string[], url: string): boolean {
  try {
    checkFetchOrigin(new URL(url), compileFetchPermission(requested), { allow: policyAllow });
    return true;
  } catch {
    return false;
  }
}

test("an origin must be in both the manifest and the policy", () => {
  const origin = "https://api.example.com";
  expect(permitted(origin, [origin], `${origin}/x`)).toBe(true);
  // In the manifest but not the policy.
  expect(permitted(origin, [], `${origin}/x`)).toBe(false);
  // In the policy but not the manifest.
  expect(permitted("https://other.example.com", [origin], `${origin}/x`)).toBe(false);
});

test("a wildcard request is still bounded by the policy", () => {
  expect(permitted("*", ["https://api.example.com"], "https://api.example.com/x")).toBe(true);
  expect(permitted("*", ["https://api.example.com"], "https://evil.example.com/x")).toBe(false);
});

test("an absent policy allowlist permits nothing", () => {
  expect(permitted("*", [], "https://api.example.com/x")).toBe(false);
});

test("a policy deny beats everything", () => {
  const origin = "https://api.example.com";
  expect(() =>
    checkFetchOrigin(new URL(`${origin}/x`), compileFetchPermission("*"), {
      allow: ["*"],
      deny: [origin],
    }),
  ).toThrow("denied by host policy");
});

test("a path in a manifest entry grants only its origin", () => {
  const compiled = compileFetchPermission("https://api.example.com/v1/only");
  expect(compiled.origins).toEqual(["https://api.example.com"]);
});

test("non-HTTP protocols are refused", () => {
  expect(() =>
    checkFetchOrigin(new URL("file:///etc/passwd"), compileFetchPermission("*"), { allow: ["*"] }),
  ).toThrow("unsupported protocol");
});

test("a malformed origin fails at manifest validation, not at request time", () => {
  expect(() =>
    validateManifest({
      name: "p",
      version: "1.0.0",
      apiVersion: 1,
      entry: "./plugin.js",
      permissions: { fetch: "not a url" },
    }),
  ).toThrow("not a URL");
});

test("the manifest accepts the capability flags", () => {
  const manifest = validateManifest({
    name: "p",
    version: "1.0.0",
    apiVersion: 1,
    entry: "./plugin.js",
    permissions: { crypto: true, timers: true, fetch: ["https://api.example.com"] },
  });
  expect(manifest.permissions?.crypto).toBe(true);
  expect(manifest.permissions?.timers).toBe(true);
  expect(manifest.permissions?.fetch).toEqual(["https://api.example.com"]);
});

test("a non-boolean crypto flag is rejected", () => {
  expect(() =>
    validateManifest({
      name: "p",
      version: "1.0.0",
      apiVersion: 1,
      entry: "./plugin.js",
      permissions: { crypto: "yes" },
    }),
  ).toThrow("must be a boolean");
});

// --- napi:fetch: end to end -------------------------------------------------

/// A stub transport. Typed loosely because the tests only exercise the one
/// call shape the capability makes.
type Transport = (url: string, init?: unknown) => Promise<Response>;

/// The capability needs an async host call, which parks the VM thread — so it
/// runs through `runAsync`, not `run`. Its only transport is the global
/// `fetch`, stubbed per test.
function fetchVm(requested: unknown, allow: string[]): Vm {
  const vm = new Vm();
  installForTest("fetch", vm, {
    manifestPermissions: { fetch: requested },
    grant: { allow },
  });
  return vm;
}

/** Run with a stubbed global fetch; always restored. */
async function withFetchStub(transport: Transport, fn: () => Promise<void>): Promise<void> {
  const previous = globalThis.fetch;
  globalThis.fetch = transport as typeof fetch;
  try {
    await fn();
  } finally {
    globalThis.fetch = previous;
  }
}

test("a permitted request reaches the transport", async () => {
  const seen: string[] = [];
  await withFetchStub(async (url) => {
    seen.push(String(url));
    return new Response("hello", { status: 200 });
  }, async () => {
    const vm = fetchVm("https://api.example.com", ["https://api.example.com"]);
    const result = await vm.runAsync(
      "import { fetch } from 'napi:fetch'; const r = await fetch('https://api.example.com/x'); r.status + ':' + r.text();",
    );
    expect(result).toBe("200:hello");
    expect(seen).toEqual(["https://api.example.com/x"]);
    vm.dispose();
  });
});

test("a denied origin never reaches the transport", async () => {
  let called = false;
  await withFetchStub(async () => {
    called = true;
    return new Response("", { status: 200 });
  }, async () => {
    const vm = fetchVm("https://api.example.com", ["https://api.example.com"]);
    const result = await vm.runAsync(
      "import { fetch } from 'napi:fetch'; try { await fetch('https://evil.example.com/x'); 'allowed'; } catch (e) { 'denied'; }",
    );
    expect(result).toBe("denied");
    expect(called).toBe(false);
    vm.dispose();
  });
});

test("json() parses the body", async () => {
  await withFetchStub(async () => new Response('{"a":1}', { status: 200 }), async () => {
    const vm = fetchVm("https://api.example.com", ["https://api.example.com"]);
    expect(
      await vm.runAsync(
        "import { fetch } from 'napi:fetch'; const r = await fetch('https://api.example.com/x'); r.json().a;",
      ),
    ).toBe("1");
    vm.dispose();
  });
});

test("fetch is removable through its teardown", () => {
  const vm = new Vm();
  const teardown = installForTest("fetch", vm, {
    manifestPermissions: { fetch: "*" },
    grant: { allow: ["*"] },
  });
  teardown();
  expect(() => vm.run("import { fetch } from 'napi:fetch'; fetch;")).toThrow();
});
