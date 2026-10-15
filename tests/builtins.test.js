import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";

test("Math.PI", () => {
  expect(runCode("Math.PI;")).toBe("3.141592653589793");
});

test("Math.E", () => {
  expect(runCode("Math.E;")).toBe("2.718281828459045");
});

test("Math constants", () => {
  expect(runCode("Math.LN2;")).toContain("0.693");
  expect(runCode("Math.LN10;")).toContain("2.302");
  expect(runCode("Math.LOG2E;")).toContain("1.442");
  expect(runCode("Math.LOG10E;")).toContain("0.434");
  expect(runCode("Math.SQRT1_2;")).toContain("0.707");
  expect(runCode("Math.SQRT2;")).toContain("1.414");
});

test("JSON exists", () => {
  expect(runCode("typeof JSON;")).toBe("object");
});

test("JSON has parse and stringify", () => {
  const vm = new Vm();
  expect(vm.run("'parse' in JSON;")).toBe("true");
  expect(vm.run("'stringify' in JSON;")).toBe("true");
});

test("Object exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'keys' in Object;")).toBe("true");
  expect(vm.run("'values' in Object;")).toBe("true");
  expect(vm.run("'entries' in Object;")).toBe("true");
  expect(vm.run("'assign' in Object;")).toBe("true");
});

test("Array exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'isArray' in Array;")).toBe("true");
  expect(vm.run("'from' in Array;")).toBe("true");
  expect(vm.run("'of' in Array;")).toBe("true");
});

test("Promise exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'resolve' in Promise;")).toBe("true");
  expect(vm.run("'reject' in Promise;")).toBe("true");
  expect(vm.run("'all' in Promise;")).toBe("true");
  expect(vm.run("'race' in Promise;")).toBe("true");
});

test("Date exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'now' in Date;")).toBe("true");
  expect(vm.run("'parse' in Date;")).toBe("true");
  expect(vm.run("'UTC' in Date;")).toBe("true");
});

test("Number exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'isNaN' in Number;")).toBe("true");
  expect(vm.run("'isFinite' in Number;")).toBe("true");
  expect(vm.run("'parseInt' in Number;")).toBe("true");
  expect(vm.run("'parseFloat' in Number;")).toBe("true");
});

test("String exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'fromCharCode' in String;")).toBe("true");
});

test("Symbol exists with iterator", () => {
  const vm = new Vm();
  expect(vm.run("typeof Symbol.iterator;")).toBe("symbol");
});

test("Reflect exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'apply' in Reflect;")).toBe("true");
  expect(vm.run("'construct' in Reflect;")).toBe("true");
  expect(vm.run("'get' in Reflect;")).toBe("true");
  expect(vm.run("'set' in Reflect;")).toBe("true");
  expect(vm.run("'has' in Reflect;")).toBe("true");
});

test("unsupported Intl formatters are absent", () => {
  const vm = new Vm();
  expect(vm.run("'DateTimeFormat' in Intl;")).toBe("false");
  expect(vm.run("'NumberFormat' in Intl;")).toBe("false");
});

test("ArrayBuffer exists with isView", () => {
  const vm = new Vm();
  expect(vm.run("'isView' in ArrayBuffer;")).toBe("true");
});

test("BigInt exists with members", () => {
  const vm = new Vm();
  expect(vm.run("'asIntN' in BigInt;")).toBe("true");
  expect(vm.run("'asUintN' in BigInt;")).toBe("true");
});

for (const name of ["console", "process", "Buffer", "crypto", "navigator", "performance", "location", "localStorage", "URL", "WebSocket", "Response", "module", "history", "screen"]) {
  test(`${name} is absent from the default engine`, () => {
    expect(runCode(`typeof ${name};`)).toBe("undefined");
  });
}

test("encodeURI leaves reserved characters alone", () => {
  expect(runCode("encodeURI('https://example.com/a b?x=1&y=2#frag');")).toBe(
    "https://example.com/a%20b?x=1&y=2#frag"
  );
  expect(runCode("encodeURI(';,/?:@&=+$#');")).toBe(";,/?:@&=+$#");
});

test("encodeURIComponent escapes reserved characters", () => {
  expect(runCode("encodeURIComponent(';,/?:@&=+$#');")).toBe(
    "%3B%2C%2F%3F%3A%40%26%3D%2B%24%23"
  );
  expect(runCode("encodeURIComponent(\"'-_.!~*'()\" );")).toBe("'-_.!~*'()");
});

test("URI codecs handle non-ASCII via UTF-8", () => {
  expect(runCode("encodeURIComponent('é');")).toBe("%C3%A9");
  expect(runCode("encodeURIComponent('€');")).toBe("%E2%82%AC");
  expect(runCode("encodeURIComponent('😀');")).toBe("%F0%9F%98%80");
  expect(runCode("decodeURIComponent('%C3%A9');")).toBe("é");
  expect(runCode("decodeURIComponent('%E2%82%AC');")).toBe("€");
  expect(runCode("decodeURIComponent('%F0%9F%98%80');")).toBe("😀");
});

test("decodeURI keeps reserved triplets encoded", () => {
  expect(runCode("decodeURI('%23');")).toBe("%23");
  expect(runCode("decodeURIComponent('%23');")).toBe("#");
  expect(runCode("decodeURIComponent('%41');")).toBe("A");
});

test("Math.f16round rounds to half precision", () => {
  expect(runCode("Math.f16round(1.0005);")).toBe("1");
  expect(runCode("Math.f16round(0.1);")).toBe("0.0999755859375");
  expect(runCode("Math.f16round.length;")).toBe("1");
  expect(runCode("String(Math.f16round(NaN));")).toBe("NaN");
});

test("malformed URIs throw URIError", () => {
  expect(runCode("try { decodeURI('%'); } catch(e) { e.name; }")).toBe("URIError");
  expect(runCode("try { decodeURI('%ZZ'); } catch(e) { e.name; }")).toBe("URIError");
  expect(runCode("try { decodeURI('%C0%AF'); } catch(e) { e.name; }")).toBe("URIError");
  expect(runCode("try { decodeURI('%ED%A0%80'); } catch(e) { e.name; }")).toBe("URIError");
  expect(runCode("try { encodeURI('\\uD800'); } catch(e) { e.name; }")).toBe("URIError");
  expect(
    runCode("decodeURIComponent('%ED%A0%80%ED%B0%80') === '\\uD800\\uDC00';")
  ).toBe("true");
});
