/**
 * Node-native compatibility smoke tests.
 *
 * The main suite runs under Bun, which loads the `.node` binary and resolves
 * CommonJS/ESM with its own implementations. That leaves the package's actual
 * runtime claim -- the `engines.node` range -- untested. This file runs under
 * `node --test` across the supported matrix and exercises the boundaries that
 * differ between runtimes: native module loading, CJS and ESM entry points,
 * and the N-API marshalling in both directions.
 *
 * It is deliberately small. It is a compatibility gate, not a second copy of
 * the language suite.
 */

const test = require("node:test");
const assert = require("node:assert");
const path = require("node:path");

const root = path.join(__dirname, "..", "..");

// ── loading ──────────────────────────────────────────────────────────

test("the CommonJS entry point loads the native binding", () => {
  const api = require(path.join(root, "index.js"));
  assert.strictEqual(typeof api.Vm, "function");
  assert.strictEqual(typeof api.runCode, "function");
});

test("the ESM entry point loads the native binding", async () => {
  const url = new URL("../../index.mjs", `file://${__filename}`);
  const api = await import(url.href);
  assert.strictEqual(typeof api.Vm, "function");
});

test("the generated entry points agree on the package version", () => {
  const fs = require("node:fs");
  const pkg = require(path.join(root, "package.json"));
  for (const file of ["index.js", "index.mjs"]) {
    const source = fs.readFileSync(path.join(root, file), "utf8");
    const versions = new Set(source.match(/\d+\.\d+\.\d+/g) ?? []);
    assert.ok(
      versions.size === 0 || versions.has(pkg.version),
      `${file} references ${[...versions].join(", ")} but package is ${pkg.version}`,
    );
  }
});

// ── evaluation and marshalling ───────────────────────────────────────

test("basic evaluation works", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  assert.strictEqual(vm.run("1 + 1;"), "2");
  assert.strictEqual(vm.run("[1,2,3].map(x => x * 2).join(',');"), "2,4,6");
});

test("values marshal in both directions", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.setGlobal("config", { name: "n", nested: { list: [1, 2, 3] } });
  assert.strictEqual(vm.run("config.nested.list.length;"), "3");
  vm.run("function join(a, b) { return a + ':' + b; }");
  assert.strictEqual(vm.callFunction("join", ["a", "b"]), "a:b");
});

test("host functions are callable from the guest", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.exposeFunction("double", (n) => n * 2);
  assert.strictEqual(vm.run("double(21);"), "42");
});

test("async execution settles its promise", async () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.exposeAsyncFunction("later", async (n) => n + 1);
  assert.strictEqual(await vm.runAsync("await later(1);"), "2");
  vm.dispose();
});

// ── process lifetime ─────────────────────────────────────────────────
//
// A native threadsafe-function keeps the N-API environment alive, so these
// assert that using the VM does not silently make a script un-exitable. They
// run as subprocesses because that is the only way to observe process exit.

const { execFileSync } = require("node:child_process");

function runsToCompletion(source, timeoutMs = 15000) {
  execFileSync(process.execPath, ["-e", source], {
    timeout: timeoutMs,
    stdio: "pipe",
    cwd: root,
  });
}

test("exposing an async function does not keep the process alive", () => {
  runsToCompletion(`
    const { Vm } = require("./index.js");
    const vm = new Vm();
    vm.exposeAsyncFunction("g", async () => 1);
  `);
});

test("registering an async host module does not keep the process alive", () => {
  runsToCompletion(`
    const { Vm } = require("./index.js");
    const vm = new Vm();
    vm.registerHostModule("m", { g: async () => 1 }, { async: ["g"] });
  `);
});

test("a synchronous run does not keep the process alive", () => {
  runsToCompletion(`
    const { Vm } = require("./index.js");
    const vm = new Vm();
    vm.exposeFunction("f", () => 1);
    if (vm.run("f();") !== "1") process.exit(3);
  `);
});

test("dispose() lets a process that used runAsync exit", () => {
  runsToCompletion(`
    const { Vm } = require("./index.js");
    const vm = new Vm();
    vm.exposeAsyncFunction("later", async (n) => n + 1);
    (async () => {
      if (await vm.runAsync("await later(1);") !== "2") process.exit(3);
      vm.dispose();
    })();
  `);
});

test("dispose() is idempotent and safe before any async work", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.dispose();
  vm.dispose();
  // A disposed VM still evaluates plain guest code.
  assert.strictEqual(vm.run("1 + 1;"), "2");
});

// ── error and limit boundaries ───────────────────────────────────────

test("guest errors surface as host exceptions", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  assert.throws(() => vm.run('throw new Error("boom");'), /boom/);
});

test("the loop budget interrupts an infinite loop", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.setLoopLimit(1000);
  assert.throws(() => vm.run("while (true) {}"), /RangeError/);
});

test("recursion depth is capped rather than crashing the process", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  const nodeVm = require("node:vm");
  const boundedSource = "function c(n) { return n <= 0 ? 0 : 1 + c(n - 1); } c(200);";
  assert.strictEqual(
    vm.run(boundedSource),
    String(nodeVm.runInNewContext(boundedSource)),
  );
  const caughtOverflowSource = "function f() { return f(); } try { f(); } catch (e) { e.name; }";
  assert.strictEqual(vm.run(caughtOverflowSource), nodeVm.runInNewContext(caughtOverflowSource));
  assert.throws(() => vm.run("function f() { return f(); } f();"), /RangeError/);
});

// ── module registry ──────────────────────────────────────────────────

test("removeModule makes a module unresolvable", () => {
  const { Vm } = require(path.join(root, "index.js"));
  const vm = new Vm();
  vm.registerModule("dep", "export const val = 1;");
  assert.strictEqual(vm.run('import { val } from "dep"; val;'), "1");

  vm.removeModule("dep");
  assert.strictEqual(vm.hasModule("dep"), false);
  assert.throws(() => vm.run('import { val } from "dep"; val;'), /Module not found/);
});

test("the plugin filesystem backend enforces its read limit", () => {
  const fs = require("node:fs");
  const os = require("node:os");
  // The plugin host is TypeScript; only exercise it where a build exists.
  let createNodeFileSystem;
  try {
    ({ createNodeFileSystem } = require(path.join(root, "dist", "plugins", "index.js")));
  } catch {
    return; // Not built for Node; the Bun suite covers this path.
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "napi-vm-node-"));
  const file = path.join(dir, "big.txt");
  fs.writeFileSync(file, "x".repeat(4096));
  const backend = createNodeFileSystem({ maxReadBytes: 1024 });
  assert.throws(() => backend.readText(file), /read limit/);
  fs.rmSync(dir, { recursive: true, force: true });
});
