import { test, expect } from "bun:test";
import { runCode } from "../index.js";

// ---------------------------------------------------------------------------
// The built-in namespaces are callable as well as being property bags:
// `String` is both `String.fromCharCode` and `String(x)`.
// ---------------------------------------------------------------------------

test("String coerces to a string", () => {
  expect(runCode("String(42);")).toBe("42");
  expect(runCode("String(null);")).toBe("null");
  expect(runCode("String([1, 2]);")).toBe("1,2");
});

test("typeof String is function", () => {
  expect(runCode("typeof String;")).toBe("function");
});

test("String still carries its statics", () => {
  expect(runCode("String.fromCharCode(65, 66);")).toBe("AB");
});

test("Number coerces to a number", () => {
  expect(runCode("Number('42');")).toBe("42");
  expect(runCode("Number('');")).toBe("0");
  expect(runCode("Number(true);")).toBe("1");
});

test("Boolean coerces to a boolean", () => {
  expect(runCode("Boolean(0);")).toBe("false");
  expect(runCode("Boolean('x');")).toBe("true");
});

test("Number constants", () => {
  expect(runCode("Number.MAX_SAFE_INTEGER;")).toBe("9007199254740991");
  expect(runCode("Number.isInteger(3);")).toBe("true");
  expect(runCode("Number.isInteger(3.5);")).toBe("false");
  expect(runCode("Number.isSafeInteger(2 ** 53);")).toBe("false");
});

test("Object() wraps a nullish value", () => {
  expect(runCode("typeof Object();")).toBe("object");
  expect(runCode("const o = { a: 1 }; Object(o) === o;")).toBe("true");
});

test("Object() boxes primitives and preserves their value", () => {
  expect(runCode("typeof Object(42) + ':' + Object(42).valueOf();")).toBe("object:42");
  expect(runCode("Object('abc').length + ':' + Object('abc')[1];")).toBe("3:b");
  expect(runCode("Object('abc').toUpperCase();")).toBe("ABC");
  expect(runCode("Object(true).toString();")).toBe("true");
  expect(runCode("Object(13n).toString();")).toBe("13");
});

// --- Array ------------------------------------------------------------------

test("Array.of collects its arguments", () => {
  expect(runCode("Array.of(1, 2, 3).join();")).toBe("1,2,3");
});

test("Array.from copies an array", () => {
  expect(runCode("Array.from([1, 2, 3]).join();")).toBe("1,2,3");
});

test("Array.from maps as it copies", () => {
  expect(runCode("Array.from([1, 2, 3], (x) => x * 2).join();")).toBe("2,4,6");
});

test("Array.from reads an array-like", () => {
  expect(runCode("Array.from({ length: 3 }, (_, i) => i).join();")).toBe("0,1,2");
});

test("Array.from iterates a string", () => {
  expect(runCode("Array.from('abc').join();")).toBe("a,b,c");
});

test("Array.from drains a generator", () => {
  expect(
    runCode("function* g() { yield 1; yield 2; } Array.from(g()).join();"),
  ).toBe("1,2");
});

test("Array(n) allocates n slots", () => {
  expect(runCode("Array(3).length;")).toBe("3");
});

test("Array with several arguments collects them", () => {
  expect(runCode("Array(1, 2).join();")).toBe("1,2");
});

// --- Named properties on arrays ---------------------------------------------

test("an array carries named properties", () => {
  expect(runCode("const a = [1, 2]; a.total = 9; a.total + ':' + a.length;")).toBe("9:2");
});

test("assigning length truncates", () => {
  expect(runCode("const a = [1, 2, 3]; a.length = 1; a.join();")).toBe("1");
});

// --- Tagged templates -------------------------------------------------------

test("a tag receives the chunks and the values", () => {
  expect(
    runCode("function t(s, ...v) { return s.join('|') + '#' + v.join(); } t`a${1}b${2}c`;"),
  ).toBe("a|b|c#1,2");
});

test("a tag receives the raw chunks", () => {
  expect(runCode("function t(s) { return s.raw[0]; } t`a\\nb`;")).toBe("a\\nb");
});

test("a tag receives the cooked chunks", () => {
  expect(runCode("function t(s) { return s[0].length; } t`a\\nb`;")).toBe("3");
});

test("String.raw leaves escapes alone", () => {
  expect(runCode("String.raw`a\\nb`;")).toBe("a\\nb");
});

test("String.raw interleaves substitutions", () => {
  expect(runCode("String.raw`a${1}b`;")).toBe("a1b");
});

test("a method can be a tag", () => {
  expect(
    runCode("const o = { n: 5, t(s) { return this.n + s[0]; } }; o.t`x`;"),
  ).toBe("5x");
});
