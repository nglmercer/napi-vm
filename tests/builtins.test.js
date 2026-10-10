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
