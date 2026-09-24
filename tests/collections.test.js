import { test, expect } from "bun:test";
import { spawnSync } from "node:child_process";
import { runCode } from "../index.js";

// ---------------------------------------------------------------------------
// `Map`, `Set`, `WeakMap`, `WeakSet`. Keys compare by SameValueZero, so an
// object key is matched by identity and `NaN` matches itself.
// ---------------------------------------------------------------------------

test("typeof Map and Set are functions", () => {
  expect(runCode("typeof Map;")).toBe("function");
  expect(runCode("typeof Set;")).toBe("function");
});

// --- Map --------------------------------------------------------------------

test("a map stores and reads", () => {
  expect(runCode("const m = new Map(); m.set('a', 1); m.get('a');")).toBe("1");
});

test("a map is seeded from entries", () => {
  expect(runCode("new Map([['a', 1], ['b', 2]]).size;")).toBe("2");
});

test("size tracks mutation", () => {
  expect(runCode("const m = new Map(); m.set('a', 1); m.set('b', 2); m.delete('a'); m.size;")).toBe(
    "1",
  );
});

test("has reports membership", () => {
  expect(runCode("const m = new Map(); m.set(1, 'x'); m.has(1) + ':' + m.has(2);")).toBe(
    "true:false",
  );
});

test("get on a missing key is undefined", () => {
  expect(runCode("String(new Map().get('nope'));")).toBe("undefined");
});

test("set is chainable", () => {
  expect(runCode("const m = new Map(); m.set('a', 1).set('b', 2); m.size;")).toBe("2");
});

test("an object key matches by identity", () => {
  expect(runCode("const k = {}; const m = new Map(); m.set(k, 1); m.get(k);")).toBe("1");
});

test("distinct objects are distinct keys", () => {
  expect(runCode("const m = new Map(); m.set({}, 1); String(m.get({}));")).toBe("undefined");
});

test("re-setting a key keeps its position", () => {
  expect(
    runCode("const m = new Map([['a', 1], ['b', 2]]); m.set('a', 9); [...m.keys()].join();"),
  ).toBe("a,b");
});

test("clear empties the map", () => {
  expect(runCode("const m = new Map([['a', 1]]); m.clear(); m.size;")).toBe("0");
});

test("keys, values and entries iterate", () => {
  expect(runCode("const m = new Map([['a', 1], ['b', 2]]); [...m.keys()].join();")).toBe("a,b");
  expect(runCode("const m = new Map([['a', 1], ['b', 2]]); [...m.values()].join();")).toBe("1,2");
  expect(
    runCode("const m = new Map([['a', 1]]); [...m.entries()][0].join('=');"),
  ).toBe("a=1");
});

test("a map iterates as entries", () => {
  expect(
    runCode("const m = new Map([[1, 'a'], [2, 'b']]); const o = []; for (const [k, v] of m) o.push(k + v); o.join();"),
  ).toBe("1a,2b");
});

test("forEach receives value, key and the map", () => {
  expect(
    runCode("const m = new Map([['a', 1]]); let out; m.forEach((v, k) => { out = k + v; }); out;"),
  ).toBe("a1");
});

test("map internals are not own properties", () => {
  expect(runCode("const m = new Map(); m.set('a', 1); Object.keys(m).length;")).toBe("0");
});

test("a map has no JSON representation of its entries", () => {
  expect(runCode("JSON.stringify(new Map([['a', 1]]));")).toBe("{}");
});

test("collection String tags match Node and Bun", () => {
  const source = `JSON.stringify([
    String(new Map([[1, 2]])),
    String(new Set([1])),
    String(new WeakMap()),
    String(new WeakSet()),
    Object.prototype.toString.call(new Map()),
    String({ [Symbol.toStringTag]: 'Cache' }),
  ])`;
  const result = runCode(`${source};`);
  const expected = JSON.stringify([
    "[object Map]",
    "[object Set]",
    "[object WeakMap]",
    "[object WeakSet]",
    "[object Map]",
    "[object Cache]",
  ]);

  expect(result).toBe(expected);
  for (const runtime of ["node", "bun"]) {
    const reference = spawnSync(
      runtime,
      ["-e", `process.stdout.write(${source})`],
      { encoding: "utf8" },
    );
    if (reference.error?.code === "ENOENT") continue;
    expect(reference.status).toBe(0);
    expect(reference.stdout).toBe(result);
  }
});

// --- Set --------------------------------------------------------------------

test("a set deduplicates", () => {
  expect(runCode("new Set([1, 2, 2, 3]).size;")).toBe("3");
});

test("a set spreads to its values", () => {
  expect(runCode("[...new Set([1, 2])].join();")).toBe("1,2");
});

test("add is chainable and idempotent", () => {
  expect(runCode("const s = new Set(); s.add(1).add(1); s.size;")).toBe("1");
});

test("NaN is its own match", () => {
  expect(runCode("const s = new Set(); s.add(NaN); s.add(NaN); s.size;")).toBe("1");
});

test("a set is seeded from a string", () => {
  expect(runCode("new Set('abc').size;")).toBe("3");
});

test("delete reports whether it removed anything", () => {
  expect(runCode("const s = new Set([1]); s.delete(1) + ':' + s.delete(1);")).toBe("true:false");
});

test("set entries pair each value with itself", () => {
  expect(runCode("[...new Set([1])][0];")).toBe("1");
  expect(runCode("[...new Set([1]).entries()][0].join(':');")).toBe("1:1");
});

test("forEach sums a set", () => {
  expect(runCode("const s = new Set([1, 2]); let t = 0; s.forEach((v) => { t += v; }); t;")).toBe(
    "3",
  );
});

// --- WeakMap / WeakSet ------------------------------------------------------

test("a weak map stores by identity", () => {
  expect(runCode("const w = new WeakMap(); const k = {}; w.set(k, 5); w.get(k);")).toBe("5");
});

test("a weak set holds membership", () => {
  expect(runCode("const w = new WeakSet(); const k = {}; w.add(k); w.has(k);")).toBe("true");
});

test("a weak collection has no size", () => {
  expect(runCode("String(new WeakMap().size);")).toBe("undefined");
});

// --- for...of destructuring -------------------------------------------------

test("for...of destructures an array pattern", () => {
  expect(
    runCode("const o = []; for (const [a, b] of [[1, 2], [3, 4]]) o.push(a + b); o.join();"),
  ).toBe("3,7");
});

test("for...of destructures an object pattern", () => {
  expect(runCode("const o = []; for (const { id } of [{ id: 1 }, { id: 2 }]) o.push(id); o.join();")).toBe(
    "1,2",
  );
});
