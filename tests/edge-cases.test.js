import { test, expect } from "bun:test";
import { Vm, runCode } from "../index.js";

test("class declaration", () => {
  expect(runCode("class Foo {} typeof Foo;")).toBe("function");
});

test("class with constructor", () => {
  expect(runCode("class Point { constructor(x, y) { this.x = x; this.y = y; } } typeof Point;")).toBe("function");
});

test("class with methods", () => {
  expect(runCode("class Calc { add(a, b) { return a + b; } } typeof Calc;")).toBe("function");
});

test("class with static method", () => {
  expect(runCode("class Util { static create() { return 1; } } typeof Util;")).toBe("function");
});

test("class with fields", () => {
  expect(runCode("class Config { debug = true; } typeof Config;")).toBe("function");
});

test("class extends", () => {
  expect(runCode("class Base {} class Child extends Base {} typeof Child;")).toBe("function");
});

test("deeply nested expressions", () => {
  expect(runCode("((((((1 + 2))))));")).toBe("3");
});

test("deeply nested function calls", () => {
  expect(runCode("function id(x) { return x; } id(id(id(id(42))));")).toBe("42");
});

test("deeply nested member access", () => {
  expect(runCode("const o = {a: {b: {c: 42}}}; o.a;")).toContain("b");
});

test("empty array", () => {
  expect(runCode("[].length;")).toBe("0");
});

test("empty object", () => {
  expect(runCode("const o = {}; typeof o;")).toBe("object");
});

test("array with undefined holes", () => {
  expect(runCode("[1, , 3];")).toBe("[1, undefined, 3]");
});

test("string with special chars", () => {
  expect(runCode("'hello\\tworld';")).toBe("hello\tworld");
  expect(runCode("'line1\\nline2';")).toBe("line1\nline2");
});

test("number edge cases", () => {
  expect(runCode("0;")).toBe("0");
  expect(runCode("-0;")).toBe("-0");
});

test("boolean coercion in if", () => {
  expect(runCode("if (1) { 'truthy'; } else { 'falsy'; }")).toBe("truthy");
  expect(runCode("if (0) { 'truthy'; } else { 'falsy'; }")).toBe("falsy");
  expect(runCode("if ('') { 'truthy'; } else { 'falsy'; }")).toBe("falsy");
  expect(runCode("if ('x') { 'truthy'; } else { 'falsy'; }")).toBe("truthy");
  expect(runCode("if (null) { 'truthy'; } else { 'falsy'; }")).toBe("falsy");
  expect(runCode("if (undefined) { 'truthy'; } else { 'falsy'; }")).toBe("falsy");
  expect(runCode("if ([]) { 'truthy'; } else { 'falsy'; }")).toBe("truthy");
  expect(runCode("if ({}) { 'truthy'; } else { 'falsy'; }")).toBe("truthy");
});

test("boolean coercion in while", () => {
  expect(runCode("let r = ''; let x = 3; while (x) { r += x; x--; } r;")).toBe("321");
});

test("boolean coercion in ternary", () => {
  expect(runCode("0 ? 'yes' : 'no';")).toBe("no");
  expect(runCode("'' ? 'yes' : 'no';")).toBe("no");
  expect(runCode("[] ? 'yes' : 'no';")).toBe("yes");
});

test("string comparison via ==", () => {
  expect(runCode("'abc' == 'abc';")).toBe("true");
  expect(runCode("'abc' == 'def';")).toBe("false");
});

test("object equality is by reference (always false)", () => {
  expect(runCode("const a = {x: 1}; const b = {x: 1}; a === b;")).toBe("false");
});

test("array equality is by reference (always false)", () => {
  expect(runCode("const a = [1]; const b = [1]; a === b;")).toBe("false");
});

test("function string coercion observes an own toString", () => {
  expect(runCode("function foo() {} foo.toString = () => 'custom function'; '' + foo;")).toBe("custom function");
});

test("native functions expose the Function object tag", () => {
  const vm = new Vm();
  expect(vm.run("Object.prototype.toString.call(Math.abs);")).toBe("[object Function]");
});

test("number formatting: integers", () => {
  expect(runCode("42;")).toBe("42");
  expect(runCode("1000000;")).toBe("1000000");
});

test("number formatting: floats", () => {
  expect(runCode("3.14;")).toBe("3.14");
  expect(runCode("0.5;")).toBe("0.5");
});

test("negative numbers", () => {
  expect(runCode("-42;")).toBe("-42");
  expect(runCode("-3.14;")).toBe("-3.14");
});

test("chained member and call", () => {
  expect(runCode("const obj = {f: () => 42}; obj.f();")).toBe("42");
});

test("computed property with expression", () => {
  expect(runCode("const arr = [10, 20, 30]; const i = 1; arr[i];")).toBe("20");
});

test("assignment in expression", () => {
  expect(runCode("let x = 0; let y = (x = 5) + 1; y;")).toBe("6");
});

test("multiple assignment", () => {
  expect(runCode("let a = 1; let b = 2; a = b; b = a; a;")).toBe("2");
});

test("complex program: sum of squares", () => {
  expect(runCode(`
    function sumOfSquares(n) {
      let sum = 0;
      for (let i = 1; i <= n; i++) {
        sum += i * i;
      }
      return sum;
    }
    sumOfSquares(5);
  `)).toBe("55");
});

test("complex program: isPrime", () => {
  expect(runCode(`
    function isPrime(n) {
      if (n < 2) { return false; }
      for (let i = 2; i * i <= n; i++) {
        if (n % i === 0) { return false; }
      }
      return true;
    }
    isPrime(7);
  `)).toBe("true");
  expect(runCode(`
    function isPrime(n) {
      if (n < 2) { return false; }
      for (let i = 2; i * i <= n; i++) {
        if (n % i === 0) { return false; }
      }
      return true;
    }
    isPrime(4);
  `)).toBe("false");
});

test("complex program: map-like pattern", () => {
  expect(runCode(`
    function map(arr, fn) {
      let result = [];
      for (const x of arr) {
        result = fn(x);
      }
      return result;
    }
    map([1, 2, 3], (x) => x * 2);
  `)).toBe("6");
});

test("complex program: accumulator pattern", () => {
  expect(runCode(`
    function reduce(arr, fn, init) {
      let acc = init;
      for (const x of arr) {
        acc = fn(acc, x);
      }
      return acc;
    }
    reduce([1, 2, 3, 4], (a) => a + 1, 0);
  `)).toBe("4");
});

test("complex program: string builder", () => {
  expect(runCode(`
    function join(arr, sep) {
      let r = '';
      for (let i = 0; i < arr.length; i++) {
        if (i > 0) { r += sep; }
        r += arr[i];
      }
      return r;
    }
    join(['a', 'b', 'c'], '-');
  `)).toBe("a-b-c");
});

test("complex program: nested data access", () => {
  expect(runCode(`
    const data = {
      users: [
        { name: 'Alice', age: 30 },
        { name: 'Bob', age: 25 }
      ]
    };
    data.users;
  `)).toContain("Alice");
});

test("program with all control flow", () => {
  expect(runCode(`
    function classify(n) {
      let result = '';
      if (n > 0) {
        result = 'positive';
      } else if (n < 0) {
        result = 'negative';
      } else {
        result = 'zero';
      }
      switch (result) {
        case 'positive': return 'P';
        case 'negative': return 'N';
        default: return 'Z';
      }
    }
    classify(5) + classify(-3) + classify(0);
  `)).toBe("PNZ");
});
