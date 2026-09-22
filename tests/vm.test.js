import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";

test("basic arithmetic", () => {
  expect(runCode("2 + 2;")).toBe("4");
  expect(runCode("10 - 3;")).toBe("7");
  expect(runCode("4 * 5;")).toBe("20");
  expect(runCode("15 / 3;")).toBe("5");
  expect(runCode("10 % 3;")).toBe("1");
});

test("variables", () => {
  expect(runCode("const x = 42; x;")).toBe("42");
  expect(runCode("let x = 1; x = 2; x;")).toBe("2");
  expect(runCode("var x = 1; x = 3; x;")).toBe("3");
});

test("functions", () => {
  expect(runCode("function add(a, b) { return a + b; } add(3, 4);")).toBe("7");
  expect(runCode("const f = (x) => x * x; f(5);")).toBe("25");
  expect(runCode("const f = (x) => x + 1; f(3);")).toBe("4");
  expect(runCode("const f = function() { return 42; }; f();")).toBe("42");
});

test("objects", () => {
  expect(runCode("const obj = { name: 'Alice', age: 30 }; obj.name;")).toBe("Alice");
  expect(runCode("const obj = { a: 1, b: 2 }; obj.b;")).toBe("2");
  expect(runCode("const obj = { x: 10 }; obj['x'];")).toBe("10");
});

test("arrays", () => {
  expect(runCode("const arr = [1, 2, 3]; arr.length;")).toBe("3");
  expect(runCode("const arr = [10, 20, 30]; arr[1];")).toBe("20");
  expect(runCode("const arr = [1, 2, 3]; arr[0];")).toBe("1");
});

test("loops", () => {
  expect(runCode("let sum = 0; for (let i = 0; i < 10; i++) { sum += i; } sum;")).toBe("45");
  expect(runCode("let i = 0; while (i < 5) { i++; } i;")).toBe("5");
});

test("closures", () => {
  expect(runCode("function counter() { let n = 0; return () => ++n; } const c = counter(); c(); c(); c();")).toBe("3");
  expect(runCode("function makeAdder(x) { return (y) => x + y; } const add5 = makeAdder(5); add5(3);")).toBe("8");
});

test("recursion", () => {
  expect(runCode("function factorial(n) { return n <= 1 ? 1 : n * factorial(n-1); } factorial(5);")).toBe("120");
  expect(runCode("function fib(n) { return n <= 1 ? n : fib(n-1) + fib(n-2); } fib(10);")).toBe("55");
});

test("import.meta", () => {
  const vm = new Vm();
  expect(vm.run("import.meta.main;")).toBe("false");
  vm.setImportMetaMain(true);
  expect(vm.run("import.meta.main;")).toBe("true");
  expect(vm.run("import.meta.url;")).toBe("vm://module");
});

test("try/catch", () => {
  expect(runCode("try { throw 'oops'; } catch(e) { 'caught: ' + e; }")).toBe("caught: oops");
  expect(runCode("let r = 'ok'; try { r = 'try'; } catch(e) { r = 'catch'; } r;")).toBe("try");
});

test("ternary", () => {
  expect(runCode("true ? 'yes' : 'no';")).toBe("yes");
  expect(runCode("false ? 'yes' : 'no';")).toBe("no");
  expect(runCode("5 > 3 ? 'big' : 'small';")).toBe("big");
});

test("prefix/postfix", () => {
  expect(runCode("let i = 0; i++;")).toBe("0");
  expect(runCode("let i = 0; ++i;")).toBe("1");
  expect(runCode("let i = 0; i++; i;")).toBe("1");
  expect(runCode("let i = 0; ++i; i;")).toBe("1");
  expect(runCode("let i = 5; i--;")).toBe("5");
  expect(runCode("let i = 5; i--; i;")).toBe("4");
});

test("compound assignment", () => {
  expect(runCode("let x = 5; x += 3; x;")).toBe("8");
  expect(runCode("let x = 10; x -= 4; x;")).toBe("6");
  expect(runCode("let x = 3; x *= 2; x;")).toBe("6");
});

test("nested functions", () => {
  expect(runCode("function outer() { function inner() { return 42; } return inner(); } outer();")).toBe("42");
});

test("builtins", () => {
  expect(runCode("Math.PI;")).toContain("3.14");
  expect(runCode("Math.E;")).toContain("2.71");
  expect(runCode("typeof 42;")).toBe("number");
  expect(runCode("typeof 'hello';")).toBe("string");
  expect(runCode("typeof true;")).toBe("boolean");
  expect(runCode("typeof undefined;")).toBe("undefined");
  expect(runCode("typeof null;")).toBe("object");
});

test("web APIs exist", () => {
  const vm = new Vm();
  // Network I/O is exposed through a granted capability, not ambiently.
  expect(vm.run("typeof fetch;")).toBe("undefined");
  expect(vm.run("typeof WebSocket;")).toBe("object");
  expect(vm.run("typeof URL;")).toBe("object");
  // `Map` and `Set` are real constructors, so they report as functions.
  expect(vm.run("typeof Map;")).toBe("function");
  expect(vm.run("typeof Set;")).toBe("function");
  // `Promise` is a real constructor, so it reports as a function.
  expect(vm.run("typeof Promise;")).toBe("function");
  expect(vm.run("typeof console;")).toBe("object");
  expect(vm.run("typeof Math;")).toBe("object");
  expect(vm.run("typeof JSON;")).toBe("object");
  expect(vm.run("typeof Date;")).toBe("function");
  expect(vm.run("typeof RegExp;")).toBe("function");
  expect(vm.run("typeof Error;")).toBe("function");
  expect(vm.run("typeof TypeError;")).toBe("function");
  // `ArrayBuffer` and the typed-array views are real constructors.
  expect(vm.run("typeof ArrayBuffer;")).toBe("function");
  expect(vm.run("typeof Uint8Array;")).toBe("function");
  expect(vm.run("typeof DataView;")).toBe("function");
  expect(vm.run("typeof crypto;")).toBe("object");
  expect(vm.run("typeof navigator;")).toBe("object");
  expect(vm.run("typeof self;")).toBe("object");
  expect(vm.run("typeof globalThis;")).toBe("object");
  expect(vm.run("typeof window;")).toBe("object");
});

test("comparison operators", () => {
  expect(runCode("5 > 3;")).toBe("true");
  expect(runCode("5 < 3;")).toBe("false");
  expect(runCode("5 >= 5;")).toBe("true");
  expect(runCode("5 <= 4;")).toBe("false");
  expect(runCode("5 === 5;")).toBe("true");
  expect(runCode("5 !== 3;")).toBe("true");
  expect(runCode("5 == 5;")).toBe("true");
  expect(runCode("5 === '5';")).toBe("false");
});

test("logical operators", () => {
  expect(runCode("true && false;")).toBe("false");
  expect(runCode("true || false;")).toBe("true");
  expect(runCode("!true;")).toBe("false");
  expect(runCode("!false;")).toBe("true");
});

test("string operations", () => {
  expect(runCode("'hello' + ' world';")).toBe("hello world");
  expect(runCode("'hello'.length;")).toBe("5");
});

test("sandboxed - no host access", () => {
  const vm = new Vm();
  // Should not have access to Node.js globals
  expect(vm.run("typeof require;")).toBe("object"); // defined as empty object
  expect(vm.run("typeof process;")).toBe("object"); // defined as empty object
  expect(vm.run("typeof globalThis;")).toBe("object"); // sandboxed global
});
