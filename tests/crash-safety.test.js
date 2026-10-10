import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";
import { runtimeFixture } from "./runtime-fixture.mjs";
const consoleRuntime = runtimeFixture({ console: true });

// Crash-safety regressions: every one of these used to kill the host process
// (SIGSEGV / SIGTRAP) or freeze it forever. They must stay catchable. The
// executable matrix with subprocess isolation lives in examples/crash.ts.

test("unbounded recursion throws a catchable RangeError", () => {
  const vm = new Vm();
  const r = vm.run("function f() { return f(); } try { f(); } catch (e) { e.message; }");
  expect(r).toContain("Maximum call stack size exceeded");
});

test("unbounded mutual recursion throws a catchable RangeError", () => {
  const vm = new Vm();
  const r = vm.run(
    "function a() { return b(); } function b() { return a(); } try { a(); } catch (e) { e.message; }"
  );
  expect(r).toContain("Maximum call stack size exceeded");
});

test("recursion limit still allows deep-but-bounded recursion", () => {
  const vm = new Vm();
  // Depth 200 < the 256 cap.
  expect(vm.run("function c(n) { return n <= 0 ? 0 : 1 + c(n - 1); } c(200);")).toBe("200");
});

test("deeply nested source is a parse error, not a crash", () => {
  const src = "(".repeat(10_000) + "1" + ")".repeat(10_000);
  expect(() => runCode(src)).toThrow(/Maximum parse depth exceeded/);
});

test("returning a cyclic object prints [Circular] instead of crashing", () => {
  const vm = new Vm();
  const r = vm.run("let o = {}; o.self = o; o;");
  expect(r).toContain("[Circular]");
});

test("returning a cyclic array prints [Circular] instead of crashing", () => {
  const vm = new Vm();
  const r = vm.run("let a = []; a.push(a); a;");
  expect(r).toContain("[Circular]");
});

consoleRuntime.test("explicit console.log of a cyclic array does not crash", () => {
  expect(() => consoleRuntime.run("let a = []; a.push(a); console.log(a);")).not.toThrow();
});

test("JSON.stringify of a cyclic structure throws a catchable TypeError", () => {
  const vm = new Vm();
  const r = vm.run(
    "let o = {}; o.self = o; try { JSON.stringify(o); } catch (e) { e.name + ': ' + e.message; }"
  );
  expect(r).toContain("circular structure");
});

test("JSON.stringify of a very deep structure throws a catchable RangeError", () => {
  const vm = new Vm();
  const r = vm.run(`
    let root = {}; let cur = root;
    for (let i = 0; i < 1000; i++) { cur.child = {}; cur = cur.child; }
    try { JSON.stringify(root); } catch (e) { e.message; }
  `);
  expect(r).toContain("Maximum JSON depth exceeded");
});

test("JSON.parse of a very deep document throws a catchable RangeError", () => {
  const vm = new Vm();
  const deep = "[".repeat(10_000) + "1" + "]".repeat(10_000);
  const r = vm.run(
    `try { JSON.parse(${JSON.stringify(deep)}); } catch (e) { e.message; }`
  );
  expect(r).toContain("Maximum JSON depth exceeded");
});

test("infinite loop hits the loop budget and throws", () => {
  const vm = new Vm();
  vm.setLoopLimit(1_000_000);
  expect(() => vm.run("while (true) {}")).toThrow(/Maximum loop iterations exceeded/);
});

test("loop budget refills on the next run", () => {
  const vm = new Vm();
  vm.setLoopLimit(1_000_000);
  expect(() => vm.run("while (true) {}")).toThrow();
  // A fresh execution gets a fresh budget.
  expect(vm.run("let s = 0; for (let i = 0; i < 10; i++) { s += i; } s;")).toBe("45");
});

test("array growth past the cap throws a catchable RangeError", () => {
  const vm = new Vm();
  const r = vm.run(
    "let a = []; try { for (let i = 0; i < 300000; i++) { a.push(1); } } catch (e) { e.message; }"
  );
  expect(r).toContain("Maximum array length exceeded");
});

test("indexed array assignment enforces the hard cap", () => {
  const vm = new Vm();
  const r = vm.run("let a = []; try { a[1000000000] = 1; } catch (e) { e.message; }");
  expect(r).toContain("Maximum array length exceeded");
});

test("sparse object destructuring does not materialize a huge array", () => {
  const vm = new Vm();
  expect(vm.run('let o = {}; o["1000000000"] = 1; let [x] = o; x;')).toBe("undefined");
});

test("JSON.parse enforces array and object caps", () => {
  const vm = new Vm();
  const arrayResult = vm.run(
    "try { JSON.parse('[' + '0,'.repeat(262144) + '0]'); } catch (e) { e.message; }",
  );
  expect(arrayResult).toContain("Maximum array length exceeded");

  const objectResult = vm.run(
    "let p = '\"a\":0,'; try { JSON.parse('{' + p.repeat(262144) + '\"z\":0}'); } catch (e) { e.message; }",
  );
  expect(objectResult).toContain("Maximum object property count exceeded");
});

test("ordinary String.replace enforces the string cap", () => {
  const vm = new Vm();
  const r = vm.run(
    "let s = 'x'.repeat(16 * 1024 * 1024); try { s.replace('x', 'xx'); } catch (e) { e.message; }",
  );
  expect(r).toContain("Maximum string length exceeded");
});

test("Array.join enforces the string cap before building its result", () => {
  const vm = new Vm();
  const r = vm.run(
    "let s = 'x'.repeat(16 * 1024 * 1024); try { [s, s].join(''); } catch (e) { e.message; }",
  );
  expect(r).toContain("Maximum string length exceeded");
});

test("Number.toFixed rejects unbounded precision", () => {
  const vm = new Vm();
  const r = vm.run(
    "try { (1).toFixed(17 * 1024 * 1024); } catch (e) { e.message; }",
  );
  expect(r).toContain("Maximum string length exceeded");
});

test("Object.assign enforces the object property cap", () => {
  const vm = new Vm();
  const r = vm.run(
    "let target = JSON.parse('{' + '\"k\":0,'.repeat(262143) + '\"last\":0}'); try { Object.assign(target, { overflow: 1 }); } catch (e) { e.message; }",
  );
  expect(r).toContain("Maximum object property count exceeded");
});

test("deep Array.prototype.flat is iterative and bounded", () => {
  const vm = new Vm();
  const r = vm.run(
    "let a = [0]; for (let i = 0; i < 100000; i++) { a = [a]; } a.flat(100000);",
  );
  expect(r).toBe("[0]");
});

test("Array.sort does not hold a RefCell borrow across a comparator", () => {
  const vm = new Vm();
  expect(
    vm.run("let a = [3, 2, 1]; a.sort((x, y) => { a.push(4); return x - y; }); a.length > 3;"),
  ).toBe("true");
});

test("Array.sort propagates comparator errors", () => {
  const vm = new Vm();
  expect(() => vm.run("let a = [3, 2, 1]; a.sort(() => { throw new Error('cmp'); });")).toThrow(
    /cmp/,
  );
});

test("deep prototype lookup is bounded without native recursion", () => {
  const vm = new Vm();
  // The chain is built with `setPrototypeOf` rather than by subclassing, so
  // the lookup is what hits its limit. Constructing 5000 nested subclasses
  // would exhaust the *call* budget first, in the implicit `super()` chain —
  // also bounded and catchable, but a different guard than this one names.
  const r = vm.run(
    "try { let p = {}; for (let i = 0; i < 5000; i++) { const n = {}; Object.setPrototypeOf(n, p); p = n; } p.missing; 'no error'; } catch (e) { e.message; }",
  );
  expect(r).toMatch(/Maximum prototype (chain )?depth exceeded/);
});

test("string doubling past the cap throws a catchable RangeError", () => {
  const vm = new Vm();
  const r = vm.run(
    "let s = 'x'; try { while (true) { s = s + s; } } catch (e) { e.message; }"
  );
  expect(r).toContain("Maximum string length exceeded");
});

test("return formatting cannot amplify a shared graph past the string cap", () => {
  const vm = new Vm();
  expect(() =>
    vm.run(`
      let child = [];
      for (let i = 0; i < 10000; i++) child.push(1);
      let outer = [];
      for (let i = 0; i < 2048; i++) outer.push(child);
      outer;
    `),
  ).toThrow(/Maximum string length exceeded/);
});

test("Interpreter-style string coercion is bounded for shared arrays", () => {
  const vm = new Vm();
  const r = vm.run(`
    let child = [];
    for (let i = 0; i < 10000; i++) child.push(1);
    let outer = [];
    for (let i = 0; i < 2048; i++) outer.push(child);
    try { '' + outer; } catch (e) { e.message; }
  `);
  expect(r).toContain("Maximum string length exceeded");
});

test("for-of over a maximum-sized string is lazy and Unicode-safe", () => {
  const vm = new Vm();
  expect(
    vm.run(`
      let s = 'x'.repeat(16 * 1024 * 1024);
      let first = '';
      for (const c of s) { first = c; break; }
      first;
    `),
  ).toBe("x");
  expect(vm.run(`
    s = 'x'.repeat(16 * 1024 * 1024);
    let it = s[Symbol.iterator]();
    it.next().value;
  `)).toBe("x");
  expect(vm.run(`
    let out = [];
    for (const c of 'a') out.push(c);
    out.join('|');
  `)).toBe("a");
});

// Global growth, update and deletion quotas are exercised in env.rs with a
// small test quota and an assertion that the production hard limit is intact.

test("updating an existing global does not consume binding quota", () => {
  const vm = new Vm();
  expect(vm.run(`
    let x = 0;
    for (let i = 0; i < 300000; i++) x = i;
    x;
  `)).toBe("299999");
});

test("deeply nested value builds fine and teardown does not crash", () => {
  const vm = new Vm();
  // 300k-deep nesting: construction is iterative; the old derived Drop would
  // SIGSEGV at process exit when this value is freed. If this regression
  // returns, the test process itself dies — a very loud failure.
  const r = vm.run("let a = [0]; for (let i = 0; i < 300000; i++) { a = [a]; } 'built';");
  expect(r).toBe("built");
});

test("recursive yield* is a catchable error, not fatal", () => {
  const vm = new Vm();
  // Each level of `yield*` starts a generator body on its own coroutine
  // stack with its own interpreter, so `MAX_CALL_DEPTH` never sees the
  // recursion. A dedicated nesting cap turns it into a `RangeError` -- this
  // used to recurse until some other limit tripped, taking ~11 seconds and
  // allocating an 8 MiB stack per level.
  expect(() => vm.run("function* g() { yield* g(); } let it = g(); it.next();")).toThrow(
    /RangeError: Maximum generator nesting exceeded/,
  );
  // The VM survives it and keeps working.
  expect(vm.run("'ok';")).toBe("ok");
});

test("callFunction into unbounded recursion throws a catchable JS error", async () => {
  const vm = new Vm();
  vm.run("function f() { return f(); }");
  expect(() => vm.callFunction("f", [])).toThrow(/Maximum call stack size exceeded/);
});
