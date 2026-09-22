import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";

// ---------------------------------------------------------------------------
// The web-platform globals that are pure computation, and therefore need no
// capability grant: `TextEncoder`/`TextDecoder`, `URLSearchParams` and
// `structuredClone`. The ones that reach outside the sandbox stay inert — see
// the sandbox suite.
// ---------------------------------------------------------------------------

test("the pure globals are callable", () => {
  expect(runCode("typeof TextEncoder;")).toBe("function");
  expect(runCode("typeof TextDecoder;")).toBe("function");
  expect(runCode("typeof URLSearchParams;")).toBe("function");
  expect(runCode("typeof structuredClone;")).toBe("function");
});

// --- TextEncoder / TextDecoder ----------------------------------------------

test("encode produces UTF-8 bytes", () => {
  expect(runCode("new TextEncoder().encode('ab').join();")).toBe("97,98");
  expect(runCode("new TextEncoder().encode('ab').BYTES_PER_ELEMENT;")).toBe("1");
});

test("decode reads UTF-8 bytes", () => {
  expect(runCode("new TextDecoder().decode(new Uint8Array([104, 105]));")).toBe("hi");
});

test("encode and decode round-trip beyond ASCII", () => {
  expect(runCode("new TextDecoder().decode(new TextEncoder().encode('héllo'));")).toBe("héllo");
  expect(runCode("new TextEncoder().encode('é').length;")).toBe("2");
});

test("decode accepts a buffer", () => {
  expect(
    runCode("const b = new TextEncoder().encode('hi').buffer; new TextDecoder().decode(b);"),
  ).toBe("hi");
});

// --- URLSearchParams --------------------------------------------------------

test("a query string parses", () => {
  expect(runCode("new URLSearchParams('a=1&b=2').get('a');")).toBe("1");
  expect(runCode("new URLSearchParams('?a=1').get('a');")).toBe("1");
});

test("a missing key is null", () => {
  expect(runCode("String(new URLSearchParams('a=1').get('zz'));")).toBe("null");
});

test("repeated keys are preserved", () => {
  expect(runCode("new URLSearchParams('a=1&a=2').getAll('a').join();")).toBe("1,2");
});

test("set replaces and append adds", () => {
  expect(
    runCode("const p = new URLSearchParams('a=1&a=2'); p.set('a', '3'); p.getAll('a').join();"),
  ).toBe("3");
  expect(
    runCode("const p = new URLSearchParams(); p.append('x', '1'); p.append('x', '2'); p.getAll('x').join();"),
  ).toBe("1,2");
});

test("has and delete", () => {
  expect(runCode("new URLSearchParams('a=1').has('a');")).toBe("true");
  expect(runCode("const p = new URLSearchParams('a=1'); p.delete('a'); p.has('a');")).toBe("false");
});

test("percent and plus encoding round-trip", () => {
  expect(runCode("new URLSearchParams('a=hello+world').get('a');")).toBe("hello world");
  expect(runCode("const p = new URLSearchParams(); p.set('a', 'x y'); p.toString();")).toBe("a=x+y");
  expect(runCode("const p = new URLSearchParams(); p.set('a', 'x&y'); p.toString();")).toBe(
    "a=x%26y",
  );
});

test("an array of pairs seeds the params", () => {
  expect(runCode("new URLSearchParams([['k', 'v']]).toString();")).toBe("k=v");
});

test("an object seeds the params", () => {
  expect(runCode("new URLSearchParams({ a: 1 }).get('a');")).toBe("1");
});

test("params iterate", () => {
  expect(runCode("[...new URLSearchParams('a=1&b=2').keys()].join();")).toBe("a,b");
  expect(runCode("[...new URLSearchParams('a=1&b=2').values()].join();")).toBe("1,2");
  expect(runCode("new URLSearchParams('a=1&b=2').size;")).toBe("2");
});

test("forEach visits each pair", () => {
  expect(
    runCode("const o = []; new URLSearchParams('a=1&b=2').forEach((v, k) => o.push(k + v)); o.join();"),
  ).toBe("a1,b2");
});

// --- structuredClone --------------------------------------------------------

test("a clone is deep", () => {
  expect(
    runCode("const o = { b: [1, 2] }; const c = structuredClone(o); c.b.push(3); o.b.length + ':' + c.b.length;"),
  ).toBe("2:3");
});

test("cycles are preserved", () => {
  expect(runCode("const a = { n: 1 }; a.self = a; const c = structuredClone(a); c.self === c;")).toBe(
    "true",
  );
});

test("shared references stay shared", () => {
  expect(runCode("const x = { v: 1 }; const c = structuredClone([x, x]); c[0] === c[1];")).toBe(
    "true",
  );
});

test("dates and typed arrays clone by value", () => {
  expect(runCode("structuredClone(new Date(5)).getTime();")).toBe("5");
  expect(runCode("structuredClone(new Uint8Array([1, 2])).join();")).toBe("1,2");
  expect(
    runCode("const a = new Uint8Array([1]); const c = structuredClone(a); c[0] = 9; a[0];"),
  ).toBe("1");
});

test("a function cannot be cloned", () => {
  expect(() => runCode("structuredClone(() => 1);")).toThrow("DataCloneError");
});

test("primitives pass through", () => {
  expect(runCode("structuredClone(1);")).toBe("1");
  expect(runCode("structuredClone('a');")).toBe("a");
  expect(runCode("structuredClone(1n).toString();")).toBe("1");
});

// --- The capability boundary is unchanged ----------------------------------

test("network APIs remain unavailable or inert before capabilities are granted", () => {
  const vm = new Vm();
  // These need a capability the host grants explicitly; they are deliberately
  // not ambient.
  expect(vm.run("typeof fetch;")).toBe("undefined");
  expect(vm.run("typeof Request;")).toBe("object");
  expect(vm.run("typeof WebSocket;")).toBe("object");
});
