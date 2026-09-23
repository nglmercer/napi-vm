import { test, expect } from "bun:test";
import { runCode } from "../index.js";

// ---------------------------------------------------------------------------
// Promises and the microtask queue.
//
// The point of these is *ordering*: a reaction is a microtask, so it runs
// after the synchronous code that registered it. Each expectation matches the
// same source in a real JavaScript engine.
// ---------------------------------------------------------------------------

// --- Ordering ---------------------------------------------------------------

test("a then callback runs after the synchronous code", () => {
  expect(
    runCode(
      "let o = []; o.push('a'); Promise.resolve().then(() => o.push('c')); o.push('b'); await 0; o.join('');",
    ),
  ).toBe("abc");
});

test("a then callback has not run yet at the next statement", () => {
  expect(runCode("let o = []; Promise.resolve(1).then((v) => o.push(v)); o.length;")).toBe("0");
});

test("an async function suspends at await", () => {
  expect(
    runCode(
      "let o = []; async function f() { o.push(1); await 0; o.push(3); } f(); o.push(2); await 0; await 0; o.join();",
    ),
  ).toBe("1,2,3");
});

test("chained thens run in order", () => {
  expect(
    runCode(
      "let o = []; Promise.resolve().then(() => o.push(1)).then(() => o.push(2)); Promise.resolve().then(() => o.push(3)); await 0; await 0; await 0; o.join();",
    ),
  ).toBe("1,3,2");
});

test("queueMicrotask defers", () => {
  expect(
    runCode("let o = []; queueMicrotask(() => o.push('later')); o.push('now'); await 0; o.join();"),
  ).toBe("now,later");
});

test("timers run after every microtask", () => {
  expect(
    runCode(
      "let o = []; setTimeout(() => o.push('t'), 0); Promise.resolve().then(() => o.push('m')); o.push('s'); await 0; o.join();",
    ),
  ).toBe("s,m");
});

// --- The constructor --------------------------------------------------------

test("new Promise resolves", () => {
  expect(runCode("await new Promise((resolve) => resolve(3));")).toBe("3");
});

test("new Promise rejects", () => {
  expect(
    runCode("await new Promise((_, reject) => reject(new Error('x'))).catch((e) => e.message);"),
  ).toBe("x");
});

test("a throwing executor rejects the promise", () => {
  expect(
    runCode("await new Promise(() => { throw new Error('boom'); }).catch((e) => e.message);"),
  ).toBe("boom");
});

test("an executor that is not a function throws", () => {
  expect(() => runCode("new Promise(1);")).toThrow();
});

test("a promise settles only once", () => {
  expect(
    runCode("await new Promise((resolve) => { resolve(1); resolve(2); });"),
  ).toBe("1");
});

test("a promise stays pending until resolved", () => {
  expect(
    runCode("let r; const p = new Promise((res) => { r = res; }); let out = 'pending'; p.then((v) => { out = v; }); await 0; out;"),
  ).toBe("pending");
});

test("a later resolution reaches an earlier then", () => {
  expect(
    runCode(
      "let r; const p = new Promise((res) => { r = res; }); let out; p.then((v) => { out = v; }); r(7); await 0; out;",
    ),
  ).toBe("7");
});

test("typeof Promise is function", () => {
  expect(runCode("typeof Promise;")).toBe("function");
});

// --- then / catch / finally -------------------------------------------------

test("then maps the value", () => {
  expect(runCode("await Promise.resolve(1).then((v) => v + 1);")).toBe("2");
});

test("catch handles a rejection", () => {
  expect(runCode("await Promise.reject(1).catch((v) => v + 1);")).toBe("2");
});

test("a throw inside then becomes a rejection", () => {
  expect(
    runCode("await Promise.resolve(1).then(() => { throw new Error('e'); }).catch((e) => e.message);"),
  ).toBe("e");
});

test("then without an onRejected forwards the rejection", () => {
  expect(runCode("await Promise.reject('r').then((v) => v).catch((e) => e);")).toBe("r");
});

test("catch forwards a fulfilment", () => {
  expect(runCode("await Promise.resolve('v').catch(() => 'no').then((v) => v);")).toBe("v");
});

test("finally runs and passes the value through", () => {
  expect(
    runCode("let o = []; await Promise.resolve(1).finally(() => o.push('f')).then((v) => o.push(v)); o.join();"),
  ).toBe("f,1");
});

test("finally passes a rejection through", () => {
  expect(runCode("await Promise.reject('r').finally(() => {}).catch((e) => e);")).toBe("r");
});

test("Promise.resolve is idempotent on a promise", () => {
  expect(runCode("const p = Promise.resolve(1); p === Promise.resolve(p);")).toBe("true");
});

// --- Thenable assimilation --------------------------------------------------

test("resolving with a thenable adopts its value", () => {
  expect(runCode("await Promise.resolve({ then(res) { res(42); } });")).toBe("42");
});

test("thenable resolution runs its then method as a microtask", () => {
  expect(
    runCode(
      "let events = []; const promise = Promise.resolve({ then(resolve) { events.push('then'); resolve(1); } }); events.push('sync'); await promise; events.join(',');",
    ),
  ).toBe("sync,then");
});

test("a throwing then getter rejects the promise", () => {
  expect(
    runCode(
      "const thenable = { get then() { throw new TypeError('then getter failed'); } }; await Promise.resolve(thenable).catch(error => error.message);",
    ),
  ).toBe("then getter failed");
});

test("promise resolution ignores later resolve and reject calls", () => {
  expect(
    runCode(
      "let resolveSource; let rejectOuter; const source = new Promise(resolve => { resolveSource = resolve; }); const outer = new Promise((resolve, reject) => { rejectOuter = reject; resolve({ then(resolveThenable) { resolveThenable(source); resolveThenable('second'); rejectOuter('third'); } }); }); resolveSource('first'); await outer;",
    ),
  ).toBe("first");
});

test("resolving with a rejecting thenable rejects", () => {
  expect(
    runCode("await Promise.resolve({ then(_, rej) { rej('no'); } }).catch((e) => e);"),
  ).toBe("no");
});

test("a promise resolved with a promise adopts it", () => {
  expect(runCode("await new Promise((res) => res(Promise.resolve(5)));")).toBe("5");
});

// --- Combinators ------------------------------------------------------------

test("Promise.all collects values in order", () => {
  expect(runCode("await Promise.all([1, Promise.resolve(2)]).then((a) => a.join());")).toBe("1,2");
});

test("Promise.all on an empty list resolves immediately", () => {
  expect(runCode("await Promise.all([]).then((a) => a.length);")).toBe("0");
});

test("Promise.all rejects on the first rejection", () => {
  expect(runCode("await Promise.all([Promise.reject('bad'), 1]).catch((e) => e);")).toBe("bad");
});

test("Promise.allSettled reports both outcomes", () => {
  expect(
    runCode(
      "await Promise.allSettled([Promise.resolve(1), Promise.reject(2)]).then((a) => JSON.stringify(a));",
    ),
  ).toBe('[{"status":"fulfilled","value":1},{"status":"rejected","reason":2}]');
});

test("Promise.race takes the first settlement", () => {
  expect(runCode("await Promise.race([Promise.resolve(1), Promise.resolve(2)]);")).toBe("1");
});

test("Promise.any takes the first fulfilment", () => {
  expect(runCode("await Promise.any([Promise.reject(1), Promise.resolve(2)]);")).toBe("2");
});

test("Promise.any rejects with an AggregateError when all reject", () => {
  expect(
    runCode("await Promise.any([Promise.reject(1), Promise.reject(2)]).catch((e) => e.name);"),
  ).toBe("AggregateError");
});

// --- Async functions --------------------------------------------------------

test("an async function returns a promise", () => {
  expect(runCode("async function f() { return 1; } typeof f().then;")).toBe("function");
});

test("a throwing async function rejects with the thrown value", () => {
  expect(
    runCode("async function f() { throw new Error('nope'); } await f().catch((e) => e.message);"),
  ).toBe("nope");
});

test("await unwraps a promise", () => {
  expect(runCode("async function f() { return await Promise.resolve(5); } await f();")).toBe("5");
});

test("try/catch around await catches a rejection", () => {
  expect(
    runCode(
      "await (async () => { try { await Promise.reject(new Error('E')); } catch (e) { return 'caught ' + e.message; } })();",
    ),
  ).toBe("caught E");
});

test("await inside a loop accumulates", () => {
  expect(
    runCode(
      "await (async () => { let s = 0; for (const v of [1, 2, 3]) { s += await Promise.resolve(v); } return s; })();",
    ),
  ).toBe("6");
});

test("await resolves a timer-backed promise", () => {
  expect(runCode("await new Promise((res) => setTimeout(() => res(9), 5));")).toBe("9");
});

// --- Async arrows and methods -----------------------------------------------

test("an async arrow with no parameters", () => {
  expect(runCode("await (async () => 1)();")).toBe("1");
});

test("an async arrow with one bare parameter", () => {
  expect(runCode("const f = async (x) => x * 2; await f(3);")).toBe("6");
});

test("an async arrow without parentheses", () => {
  expect(runCode("const f = async x => x + 1; await f(1);")).toBe("2");
});

test("an async object method", () => {
  expect(runCode("const o = { async m() { return 5; } }; await o.m();")).toBe("5");
});

test("an async class method", () => {
  expect(runCode("class A { async m() { return 7; } } await new A().m();")).toBe("7");
});

test("async is still usable as a property name", () => {
  expect(runCode("({ async: 1 }).async;")).toBe("1");
});

// --- for await / async generators -------------------------------------------

test("for await consumes an array of promises", () => {
  expect(
    runCode(
      "await (async () => { let s = 0; for await (const v of [Promise.resolve(1), 2]) { s += v; } return s; })();",
    ),
  ).toBe("3");
});

test("for await consumes an async generator", () => {
  expect(
    runCode(
      "await (async () => { const out = []; async function* g() { yield 1; yield 2; } for await (const v of g()) out.push(v); return out.join(); })();",
    ),
  ).toBe("1,2");
});

test("an async generator may await between yields", () => {
  expect(
    runCode(
      "await (async () => { const out = []; async function* g() { yield await Promise.resolve(1); yield 2; } for await (const v of g()) out.push(v); return out.join(); })();",
    ),
  ).toBe("1,2");
});

test("Symbol.asyncIterator drives for await", () => {
  expect(
    runCode(
      "const o = { async *[Symbol.asyncIterator]() { yield 1; yield 2; } }; await (async () => { let s = 0; for await (const v of o) s += v; return s; })();",
    ),
  ).toBe("3");
});

// --- Generator methods ------------------------------------------------------

test("a generator object method", () => {
  expect(runCode("const o = { *g() { yield 1; yield 2; } }; [...o.g()].join();")).toBe("1,2");
});

test("a generator class method", () => {
  expect(runCode("class A { *g() { yield 1; yield 2; } } [...new A().g()].join();")).toBe("1,2");
});
