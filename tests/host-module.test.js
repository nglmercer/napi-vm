import { test, expect } from "bun:test";
import { Vm } from "../index.js";

// `registerHostModule` is the generic form of `exposeFunction` +
// `registerModule`: the core bridges the functions and generates the wrapper
// module; what the functions do stays entirely on the host side.

test("registerHostModule exposes host functions as module exports", () => {
  const vm = new Vm();
  vm.registerHostModule("demo", {
    hello: (name) => `hi ${name}`,
    add: (a, b) => a + b,
  });
  vm.registerModule(
    "app",
    `import { hello, add } from "demo";
     export function go() { return hello("world") + "/" + add(2, 3); }`,
  );
  expect(vm.run(`import { go } from "app"; go();`)).toBe("hi world/5");
});

test("registerHostModule returns the globals it created", () => {
  const vm = new Vm();
  const globals = vm.registerHostModule("fs-test", { readText: () => "x" });
  // `:` is hex-encoded so two module names can never share a prefix.
  expect(globals).toEqual(["__hostmod_fs_2dtest_readText"]);
  expect(vm.hasGlobal("__hostmod_fs_2dtest_readText")).toBe(true);
});

test("removeModule revokes the module's bridge globals", () => {
  const vm = new Vm();
  const globals = vm.registerHostModule("demo", { ping: () => "pong" });
  expect(vm.hasModule("demo")).toBe(true);
  expect(vm.listModules()).toContain("demo");

  // Removing the module revokes the capability, not just the wrapper source.
  expect(vm.removeModule("demo")).toBe(true);
  expect(vm.hasModule("demo")).toBe(false);
  for (const name of globals) expect(vm.hasGlobal(name)).toBe(false);
});

test("module names that differ only in punctuation get separate namespaces", () => {
  const vm = new Vm();
  const first = vm.registerHostModule("a:b", { who: () => "colon" });
  const second = vm.registerHostModule("a/b", { who: () => "slash" });
  expect(first).not.toEqual(second);

  // Each module reaches its own bridge, so neither can call the other's.
  expect(vm.run(`${first[0]}();`)).toBe("colon");
  expect(vm.run(`${second[0]}();`)).toBe("slash");

  // Removing one must not disturb the other.
  vm.removeModule("a:b");
  for (const name of first) expect(vm.hasGlobal(name)).toBe(false);
  for (const name of second) expect(vm.hasGlobal(name)).toBe(true);
});

test("re-registering with fewer exports revokes the dropped bridge global", () => {
  const vm = new Vm();
  const before = vm.registerHostModule("fs-test", {
    read: () => "r",
    write: () => "w",
  });
  const writeGlobal = before.find((name) => name.endsWith("_write"));
  expect(vm.hasGlobal(writeGlobal)).toBe(true);

  const after = vm.registerHostModule("fs-test", { read: () => "r" });
  expect(after).not.toContain(writeGlobal);
  expect(vm.hasGlobal(writeGlobal)).toBe(false);
  expect(vm.run(`typeof ${writeGlobal};`)).toBe("undefined");

  // The surviving export still works, and `write` is gone from the module.
  vm.registerModule("app", `import { read } from "fs-test";
    export function go() { return read(); }`);
  expect(vm.run(`import { go } from "app"; go();`)).toBe("r");
  // The dropped export is gone from the module itself, not merely pointing at
  // a revoked global: importing it binds `undefined`, and calling it fails.
  vm.registerModule("bad", `import { write } from "fs-test";
    export function probe() { return typeof write; }
    export function go() { return write(); }`);
  expect(vm.run(`import { probe } from "bad"; probe();`)).toBe("undefined");
  expect(() => vm.run(`import { go } from "bad"; go();`)).toThrow(
    /TypeError: undefined is not a function/,
  );
});

test("a failed re-registration leaves the previous exports intact", () => {
  const vm = new Vm();
  const before = vm.registerHostModule("demo", { ping: () => "pong" });
  expect(() =>
    vm.registerHostModule("demo", { ping: () => "pong", bad: 1 }),
  ).toThrow(/must be a function/);
  for (const name of before) expect(vm.hasGlobal(name)).toBe(true);

  vm.registerModule("app", `import { ping } from "demo";
    export function go() { return ping(); }`);
  expect(vm.run(`import { go } from "app"; go();`)).toBe("pong");
});

test("exports receive every argument and can return objects", () => {
  const vm = new Vm();
  vm.registerHostModule("demo", {
    collect: (...args) => ({ count: args.length, args }),
  });
  vm.registerModule("app", `import { collect } from "demo";
    export function go() { return collect(1, "a", true); }`);
  expect(vm.run(`import { go } from "app"; JSON.stringify(go());`)).toBe(
    '{"count":3,"args":[1,"a",true]}',
  );
});

test("a host error thrown by an export is catchable in the guest", () => {
  const vm = new Vm();
  vm.registerHostModule("demo", {
    boom: () => {
      throw new Error("PermissionDenied: nope");
    },
  });
  vm.registerModule("app", `import { boom } from "demo";
    export function go() { try { boom(); return "no-throw"; } catch (e) { return e.message; } }`);
  expect(vm.run(`import { go } from "app"; go();`)).toBe("PermissionDenied: nope");
});

test("options.async marks exports the guest can await", async () => {
  const vm = new Vm();
  vm.registerHostModule(
    "net",
    { fetchText: async (url) => `body-of:${url}` },
    { async: ["fetchText"] },
  );
  vm.registerModule("app", `import { fetchText } from "net";
    export async function go() { return await fetchText("https://example.test"); }`);
  await expect(vm.runAsync(`import { go } from "app"; await go();`)).resolves.toBe(
    "body-of:https://example.test",
  );
});

test("re-registering a host module replaces the previous exports", () => {
  const vm = new Vm();
  vm.registerHostModule("demo", { value: () => 1 });
  vm.registerHostModule("demo", { value: () => 2 });
  vm.registerModule("app", `import { value } from "demo";
    export function go() { return value(); }`);
  expect(vm.run(`import { go } from "app"; go();`)).toBe("2");
});

test("an empty exports object is rejected", () => {
  const vm = new Vm();
  expect(() => vm.registerHostModule("demo", {})).toThrow(
    /must export at least one function/,
  );
});

test("non-function exports are rejected", () => {
  const vm = new Vm();
  expect(() => vm.registerHostModule("demo", { a: 1 })).toThrow(
    /export 'a' must be a function/,
  );
});

test("export names that are not identifiers are rejected", () => {
  const vm = new Vm();
  expect(() => vm.registerHostModule("demo", { "a-b": () => 1 })).toThrow(
    /is not a usable export name/,
  );
  expect(() => vm.registerHostModule("demo", { default: () => 1 })).toThrow(
    /is not a usable export name/,
  );
  expect(() => vm.registerHostModule("demo", { "2fast": () => 1 })).toThrow(
    /is not a usable export name/,
  );
});

test("registering a host module does not leak host globals into the guest", () => {
  // The bridge globals exist by design; the point is that the *host* object
  // itself never crosses over.
  const vm = new Vm();
  const host = { secret: "s3cret", ping: () => "pong" };
  expect(() => vm.registerHostModule("demo", host)).toThrow(
    /export 'secret' must be a function/,
  );
});

test("options.async naming a missing export is rejected", () => {
  const vm = new Vm();
  expect(() =>
    vm.registerHostModule("demo", { ping: () => "pong" }, { async: ["pong"] }),
  ).toThrow(/options.async names 'pong', which is not an export/);
});

test("a failure after a binding was replaced rolls that binding back", () => {
  // Exhausting the global-binding quota makes installing a *new* global fail
  // while replacing an existing one still succeeds — a failure that lands
  // after `read` has already been swapped for its replacement.
  const vm = new Vm();
  vm.setLoopLimit(5_000_000);
  const [readGlobal] = vm.registerHostModule("cap", { read: () => "old" });
  expect(vm.run(`${readGlobal}();`)).toBe("old");

  vm.run(`let n = 0;
    let done = false;
    while (!done) {
      try { globalThis["__fill" + n] = 1; n = n + 1; }
      catch (error) { done = true; }
    }`);

  expect(() =>
    vm.registerHostModule("cap", { read: () => "new", anotherExport: () => "x" }),
  ).toThrow(/Maximum global binding count exceeded/);

  // The old function is restored *and* still callable: its bridge handle was
  // never retired, because the swap did not commit.
  expect(vm.run(`${readGlobal}();`)).toBe("old");
  expect(vm.run(`typeof __hostmod_cap_anotherExport;`)).toBe("undefined");
});

test("keyword export names are rejected, not compiled into broken source", () => {
  const vm = new Vm();
  // Each of these lexes as a keyword, so `export function <name>` would be
  // ungrammatical. They pass a naive ASCII-identifier shape check, which is
  // why validation is derived from the lexer instead of a hand-kept list.
  const keywords = [
    "null",
    "true",
    "false",
    "undefined",
    "async",
    "await",
    "from",
    "as",
    "of",
    "get",
    "set",
    "constructor",
    "class",
    "function",
    "return",
  ];
  for (const name of keywords) {
    expect(() => vm.registerHostModule("m", { [name]: () => 1 })).toThrow(
      new RegExp(`'${name}' is not a usable export name`),
    );
    expect(vm.hasModule("m")).toBe(false);
  }

  // Keyword-adjacent names are still fine.
  expect(() =>
    vm.registerHostModule("ok", { nullish: () => 1, getter: () => 2 }),
  ).not.toThrow();
});
