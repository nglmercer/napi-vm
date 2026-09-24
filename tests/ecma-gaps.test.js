import { test, expect } from "bun:test";
import { runCode, Vm } from "../index.js";

// ---------------------------------------------------------------------------
// ECMAScript conformance suite.
//
// Each test below asserts correct JavaScript behavior across the language
// features this VM implements (operators, template literals, destructuring,
// functions, control flow, error handling, objects, classes, and the standard
// library). These originally documented the implementation gaps as `test.skip`
// entries; they are now live regression tests and should all pass.
//
// Grouped by feature area. See the evaluation report for details.
// ---------------------------------------------------------------------------

// --- Operators: bitwise -----------------------------------------------------

test("bitwise AND &", () => {
  expect(runCode("6 & 3;")).toBe("2");
});

test("bitwise OR |", () => {
  expect(runCode("6 | 1;")).toBe("7");
});

test("bitwise XOR ^", () => {
  expect(runCode("6 ^ 3;")).toBe("5");
});

test("bitwise NOT ~", () => {
  expect(runCode("~5;")).toBe("-6");
});

test("left shift <<", () => {
  expect(runCode("1 << 3;")).toBe("8");
});

test("signed right shift >>", () => {
  expect(runCode("8 >> 2;")).toBe("2");
});

test("unsigned right shift >>>", () => {
  expect(runCode("-1 >>> 0;")).toBe("4294967295");
});

// --- Operators: exponentiation / nullish / optional chaining ----------------

test("exponentiation operator **", () => {
  expect(runCode("2 ** 10;")).toBe("1024");
});

test("nullish coalescing ??", () => {
  expect(runCode("null ?? 'default';")).toBe("default");
});

test("optional chaining ?.", () => {
  expect(runCode("const o = null; o?.a;")).toBe("undefined");
});

// --- Template literals ------------------------------------------------------

test("template literal basic", () => {
  expect(runCode("`hi`;")).toBe("hi");
});

test("template literal interpolation", () => {
  expect(runCode("const n='Bob'; `hi ${n}`;")).toBe("hi Bob");
});

// --- Assignment & declarations ----------------------------------------------

test("member assignment obj.prop = value", () => {
  expect(runCode("const o = {}; o.x = 5; o.x;")).toBe("5");
});

test("compound assignment %=", () => {
  expect(runCode("let x = 10; x %= 3; x;")).toBe("1");
});

test("chained assignment a = b = 5", () => {
  expect(runCode("let a; let b; a = b = 5; a;")).toBe("5");
});

test("multiple variable declaration let a, b", () => {
  expect(runCode("let a = 1, b = 2; a + b;")).toBe("3");
});

// --- Destructuring & spread -------------------------------------------------

test("array destructuring", () => {
  expect(runCode("const [a, b] = [1, 2]; a + b;")).toBe("3");
});

test("object destructuring", () => {
  expect(runCode("const { a, b } = { a: 1, b: 2 }; a + b;")).toBe("3");
});

test("spread in array literal flattens", () => {
  expect(runCode("const a = [...[1,2], 3]; a.length;")).toBe("3");
});

test("spread in function call", () => {
  expect(runCode("Math.max(...[1,5,3]);")).toBe("5");
});

test("object spread", () => {
  expect(runCode("const o = { ...{ a: 1 }, b: 2 }; o.a;")).toBe("1");
});

// --- Functions: parameters & arguments --------------------------------------

test("arrow function single param without parens", () => {
  expect(runCode("const f = x => x * 2; f(4);")).toBe("8");
});

test("arrow function multiple params", () => {
  expect(runCode("const f = (a, b) => a + b; f(2, 3);")).toBe("5");
});

test("default parameters", () => {
  expect(runCode("function f(a = 10) { return a; } f();")).toBe("10");
});

test("rest parameters", () => {
  expect(runCode("function f(...a) { return a.length; } f(1, 2, 3);")).toBe("3");
});

test("arguments object", () => {
  expect(runCode("function f() { return arguments.length; } f(1, 2);")).toBe("2");
});

// --- Control flow -----------------------------------------------------------

test("do...while loop", () => {
  expect(runCode("let i = 0; do { i++; } while (i < 5); i;")).toBe("5");
});

test("break inside a loop", () => {
  expect(runCode("let i = 0; while (true) { if (i >= 3) { break; } i++; } i;")).toBe("3");
});

test("continue inside a loop", () => {
  expect(runCode("let s = 0; for (let i = 0; i < 5; i++) { if (i % 2) { continue; } s += i; } s;")).toBe("6");
});

test("labeled break", () => {
  expect(
    runCode("let n = 0; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j === 1) { break outer; } n++; } } n;")
  ).toBe("1");
});

test("braceless if statement", () => {
  expect(runCode("let r = 'n'; if (true) r = 'y'; r;")).toBe("y");
});

test("for...of over a string", () => {
  expect(runCode("let r = ''; for (const c of 'abc') { r += c; } r;")).toBe("abc");
});

test("comma operator / multi-init for loop", () => {
  expect(runCode("let s = 0; for (let i = 0, j = 10; i < j; i++, j--) { s++; } s;")).toBe("5");
});

// --- Error handling ---------------------------------------------------------

test("finally runs after catch", () => {
  expect(runCode("let r = ''; try { throw 'x'; } catch (e) { r += 'c'; } finally { r += 'f'; } r;")).toBe("cf");
});

test("try/catch catches runtime (non-throw) errors", () => {
  expect(runCode("let r = ''; try { undefinedVar; } catch (e) { r = 'caught'; } r;")).toBe("caught");
});

test("typeof undeclared variable is 'undefined' (no throw)", () => {
  expect(runCode("typeof notDefinedAnywhere;")).toBe("undefined");
});

// --- Objects ----------------------------------------------------------------

test("object property shorthand { x }", () => {
  expect(runCode("const x = 5; const o = { x }; o.x;")).toBe("5");
});

test("object computed property key", () => {
  expect(runCode("const k = 'a'; const o = { [k]: 1 }; o.a;")).toBe("1");
});

test("object method shorthand", () => {
  expect(runCode("const o = { f() { return 3; } }; o.f();")).toBe("3");
});

test("object getter", () => {
  expect(runCode("const o = { get x() { return 5; } }; o.x;")).toBe("5");
});

// --- Classes & OOP ----------------------------------------------------------

test("class constructor with this", () => {
  expect(runCode("class A { constructor() { this.x = 1; } } new A().x;")).toBe("1");
});

test("class instance method", () => {
  expect(runCode("class A { foo() { return 5; } } new A().foo();")).toBe("5");
});

test("class field", () => {
  expect(runCode("class A { x = 7; } new A().x;")).toBe("7");
});

test("class static method", () => {
  expect(runCode("class A { static foo() { return 9; } } A.foo();")).toBe("9");
});

test("class inheritance", () => {
  expect(runCode("class A { foo() { return 1; } } class B extends A {} new B().foo();")).toBe("1");
});

test("super call in derived constructor", () => {
  expect(
    runCode("class A { constructor() { this.x = 1; } } class B extends A { constructor() { super(); this.y = 2; } } new B().x;")
  ).toBe("1");
});

test("instanceof", () => {
  expect(runCode("class A {} new A() instanceof A;")).toBe("true");
});

test("function constructor assigns via this", () => {
  expect(runCode("function P() { this.x = 5; } new P().x;")).toBe("5");
});

// --- Standard library: Array ------------------------------------------------

test("Array.prototype.map", () => {
  expect(runCode("[1,2,3].map((x) => x * 2).length;")).toBe("3");
});

test("Array.prototype.filter", () => {
  expect(runCode("[1,2,3,4].filter((x) => x % 2 === 0).length;")).toBe("2");
});

test("Array.prototype.reduce", () => {
  expect(runCode("[1,2,3].reduce((a, b) => a + b, 0);")).toBe("6");
});

test("Array.prototype.push mutates", () => {
  expect(runCode("const a = [1]; a.push(2); a.length;")).toBe("2");
});

test("Array.prototype.join", () => {
  expect(runCode("[1,2,3].join('-');")).toBe("1-2-3");
});

test("Array.isArray", () => {
  expect(runCode("Array.isArray([1,2]);")).toBe("true");
});

test("Array.isArray follows nested proxies", () => {
  expect(runCode(
    "[Array.isArray(new Proxy([], {})), " +
    "Array.isArray(new Proxy(new Proxy([], {}), {})), " +
    "Array.isArray(new Proxy({}, {}))].join(',');",
  )).toBe("true,true,false");
});

// --- Standard library: String -----------------------------------------------

test("string index access", () => {
  expect(runCode("'abc'[1];")).toBe("b");
});

test("String.prototype.toUpperCase", () => {
  expect(runCode("'abc'.toUpperCase();")).toBe("ABC");
});

test("String.prototype.slice", () => {
  expect(runCode("'hello'.slice(1,3);")).toBe("el");
});

test("String.prototype.split", () => {
  expect(runCode("'a,b,c'.split(',').length;")).toBe("3");
});

test("String.prototype.includes", () => {
  expect(runCode("'hello'.includes('ell');")).toBe("true");
});

// --- Standard library: Number / Math ----------------------------------------

test("Math.abs", () => {
  expect(runCode("Math.abs(-5);")).toBe("5");
});

test("Math.floor", () => {
  expect(runCode("Math.floor(3.7);")).toBe("3");
});

test("Math.sqrt", () => {
  expect(runCode("Math.sqrt(16);")).toBe("4");
});

test("Math.max variadic", () => {
  expect(runCode("Math.max(1,5,3);")).toBe("5");
});

test("Number.prototype.toFixed", () => {
  expect(runCode("(3.14159).toFixed(2);")).toBe("3.14");
});

test("scientific notation literal", () => {
  expect(runCode("1e3;")).toBe("1000");
});

// --- Standard library: global functions -------------------------------------

test("parseInt", () => {
  expect(runCode("parseInt('42');")).toBe("42");
});

test("parseFloat", () => {
  expect(runCode("parseFloat('3.14');")).toBe("3.14");
});

test("global isNaN", () => {
  expect(runCode("isNaN(NaN);")).toBe("true");
});

test("Number.isNaN", () => {
  expect(runCode("Number.isNaN(NaN);")).toBe("true");
});

// --- Standard library: JSON / Object ----------------------------------------

test("JSON.stringify", () => {
  expect(runCode("JSON.stringify({a:1});")).toBe('{"a":1}');
});

test("JSON.parse", () => {
  expect(runCode("JSON.parse('{\"a\":1}').a;")).toBe("1");
});

test("Object.keys", () => {
  expect(runCode("Object.keys({a:1,b:2}).length;")).toBe("2");
});

test("Object.assign", () => {
  expect(runCode("Object.assign({a:1},{b:2}).b;")).toBe("2");
});

// --- Coercion correctness ---------------------------------------------------

test("boolean coerces to number in arithmetic", () => {
  expect(runCode("true + 1;")).toBe("2");
});

test("loose equality coerces number/string", () => {
  expect(runCode("'5' == 5;")).toBe("true");
});

test("loose equality coerces boolean", () => {
  expect(runCode("0 == false;")).toBe("true");
});

// --- Async / generators (basic support) -------------------------------------

test("async function returns a promise", () => {
  expect(runCode("async function f() { return 1; } typeof f();")).toBe("object");
});

test("promise instances support then chaining", () => {
  expect(runCode(`
    async function loadUser(id) {
      return await Promise.resolve({ id, name: "Ada" });
    }
    loadUser(42).then((user) => user.name);
    await loadUser(42).then((user) => user.id);
  `)).toBe("42");
});

test("promise catch and finally are eager and chainable", () => {
  expect(runCode(`
    Promise.reject("nope")
      .catch((reason) => reason + " handled")
      .finally(() => 1);
  `)).toBe("[object Promise]");
  expect(runCode(`await Promise.reject("nope").catch((reason) => reason + " handled");`)).toBe("nope handled");
});

test("generator function", () => {
  expect(runCode("function* g() { yield 1; } typeof g;")).toBe("function");
});

// --- Standard library: Array (extended) -------------------------------------

test("Array.prototype.sort default is lexicographic", () => {
  expect(runCode("[10, 2, 30].sort().join(',');")).toBe("10,2,30");
});

test("Array.prototype.sort with comparator", () => {
  expect(runCode("[3, 1, 2].sort((a, b) => a - b).join(',');")).toBe("1,2,3");
});

test("Array.prototype.flat default depth 1", () => {
  expect(runCode("[1, [2, [3]]].flat().length;")).toBe("3");
});

test("Array.prototype.flat with depth", () => {
  expect(runCode("[1, [2, [3]]].flat(2).join(',');")).toBe("1,2,3");
});

test("Array.prototype.flatMap maps then flattens", () => {
  expect(runCode("[1, 2, 3].flatMap((x) => [x, x * 2]).join(',');")).toBe("1,2,2,4,3,6");
});

test("Array.prototype.reduceRight", () => {
  expect(runCode("[1, 2, 3].reduceRight((a, b) => a - b, 0);")).toBe("-6");
});

// --- Standard library: Date -------------------------------------------------

test("Date.UTC at the epoch", () => {
  expect(runCode("Date.UTC(1970, 0, 1);")).toBe("0");
});

test("Date.UTC with a time component", () => {
  expect(runCode("Date.UTC(1970, 0, 1, 0, 0, 1);")).toBe("1000");
});

test("Date.parse ISO string at the epoch", () => {
  expect(runCode("Date.parse('1970-01-01T00:00:00Z');")).toBe("0");
});

test("Date.parse with fractional seconds", () => {
  expect(runCode("Date.parse('1970-01-01T00:00:00.500Z');")).toBe("500");
});

test("Date.parse applies a timezone offset", () => {
  expect(runCode("Date.parse('1970-01-01T01:00:00+01:00');")).toBe("0");
});

test("Date.now returns a number", () => {
  expect(runCode("typeof Date.now();")).toBe("number");
});

// --- Standard library: console ----------------------------------------------

test("console.log is callable and returns undefined", () => {
  expect(runCode("console.log('hello');")).toBe("undefined");
});

test("console.error is callable", () => {
  expect(runCode("console.error('boom');")).toBe("undefined");
});

// --- Standard library: Error ------------------------------------------------

test("new Error carries a message", () => {
  expect(runCode("new Error('oops').message;")).toBe("oops");
});

test("new Error carries a name", () => {
  expect(runCode("new Error('x').name;")).toBe("Error");
});

test("TypeError has its own name", () => {
  expect(runCode("new TypeError('x').name;")).toBe("TypeError");
});

test("Error with no argument has an empty message", () => {
  expect(runCode("new Error().message;")).toBe("");
});

test("typeof new Error is object", () => {
  expect(runCode("typeof new Error('x');")).toBe("object");
});

test("throw/catch preserves the Error object", () => {
  expect(runCode("try { throw new Error('bad'); } catch (e) { e.message; }")).toBe("bad");
});

test("caught Error name survives the throw", () => {
  expect(runCode("try { throw new RangeError('r'); } catch (e) { e.name; }")).toBe("RangeError");
});

// --- Async: await & Promise combinators -------------------------------------

test("await unwraps a fulfilled promise", () => {
  expect(runCode("async function f() { return 5; } await f();")).toBe("5");
});

test("await on a non-promise returns the value", () => {
  expect(runCode("await 42;")).toBe("42");
});

test("await rethrows a rejected promise", () => {
  expect(runCode("try { await Promise.reject('no'); } catch (e) { e; }")).toBe("no");
});

test("Promise.resolve", () => {
  expect(runCode("await Promise.resolve(7);")).toBe("7");
});

test("Promise.all resolves to an array", () => {
  expect(runCode("(await Promise.all([Promise.resolve(1), 2])).join(',');")).toBe("1,2");
});

test("Promise.all rejects on first rejection", () => {
  expect(
    runCode("try { await Promise.all([Promise.resolve(1), Promise.reject('x')]); } catch (e) { e; }")
  ).toBe("x");
});

test("Promise.race returns the first settled", () => {
  expect(runCode("await Promise.race([Promise.resolve('a'), Promise.resolve('b')]);")).toBe("a");
});

// --- Generators: yield / next -----------------------------------------------

test("generator next yields the first value", () => {
  expect(runCode("function* g() { yield 1; yield 2; } const it = g(); it.next().value;")).toBe("1");
});

test("generator next advances through values", () => {
  expect(runCode("function* g() { yield 1; yield 2; } const it = g(); it.next(); it.next().value;")).toBe("2");
});

test("generator reports done once exhausted", () => {
  expect(runCode("function* g() { yield 1; } const it = g(); it.next(); it.next().done;")).toBe("true");
});

test("for...of drives a generator", () => {
  expect(
    runCode("function* g() { yield 1; yield 2; yield 3; } let s = 0; for (const x of g()) { s += x; } s;")
  ).toBe("6");
});

test("generator receives parameters", () => {
  expect(
    runCode(
      "function* range(n) { let i = 0; while (i < n) { yield i; i++; } } let s = 0; for (const x of range(3)) { s += x; } s;"
    )
  ).toBe("3");
});

// --- Generators: true suspension ----------------------------------------------

test("infinite generator does not hang (true suspension)", () => {
  expect(
    runCode("function* nats() { let i = 0; while (true) { yield i; i++; } } const it = nats(); it.next(); it.next(); it.next().value;")
  ).toBe("2");
});

test("generator next(val) sends a value into the yield expression", () => {
  expect(
    runCode("function* echo() { let x = yield 1; yield x + 10; } const it = echo(); it.next(); it.next(5).value;")
  ).toBe("15");
});

test("generator with yield in a conditional", () => {
  expect(
    runCode("function* g(x) { if (x > 0) { yield 'pos'; } else { yield 'neg'; } } const it = g(1); it.next().value;")
  ).toBe("pos");
});

test("generator return value is accessible via done result", () => {
  expect(
    runCode("function* g() { yield 1; return 42; } const it = g(); it.next(); it.next().value;")
  ).toBe("42");
});

test("generator is its own iterator (Symbol.iterator)", () => {
  expect(
    runCode("function* g() { yield 1; } const it = g(); typeof it[Symbol.iterator];")
  ).toBe("function");
});

// --- Symbols ----------------------------------------------------------------

test("typeof Symbol is function", () => {
  expect(runCode("typeof Symbol;")).toBe("function");
});

test("typeof Symbol() is symbol", () => {
  expect(runCode("typeof Symbol('x');")).toBe("symbol");
});

test("Symbol.iterator is a symbol", () => {
  expect(runCode("typeof Symbol.iterator;")).toBe("symbol");
});

test("Symbol.for returns a symbol", () => {
  expect(runCode("typeof Symbol.for('key');")).toBe("symbol");
});

test("Symbol.keyFor retrieves the registry key", () => {
  expect(runCode("const s = Symbol.for('myKey'); Symbol.keyFor(s);")).toBe("myKey");
});

test("well-known symbols exist", () => {
  expect(runCode("typeof Symbol.toStringTag;")).toBe("symbol");
  expect(runCode("typeof Symbol.hasInstance;")).toBe("symbol");
  expect(runCode("typeof Symbol.asyncIterator;")).toBe("symbol");
});

// --- Iterator protocol --------------------------------------------------------

test("for...of over an object with [Symbol.iterator]", () => {
  expect(
    runCode("const obj = { [Symbol.iterator]() { let i = 0; return { next() { if (i < 3) { return { value: i++, done: false }; } return { value: undefined, done: true }; } }; } }; let s = 0; for (const x of obj) { s += x; } s;")
  ).toBe("3");
});

test("array [Symbol.iterator] returns a working iterator", () => {
  expect(
    runCode("const it = [10, 20, 30][Symbol.iterator](); it.next().value + it.next().value;")
  ).toBe("30");
});

test("string [Symbol.iterator] iterates characters", () => {
  expect(
    runCode("const it = 'hi'[Symbol.iterator](); it.next().value + it.next().value;")
  ).toBe("hi");
});

// --- Modules: exports reach importers ---------------------------------------

test("named export reaches an importer", () => {
  const vm = new Vm();
  vm.registerModule("math", "export const add = (a, b) => a + b;");
  expect(vm.run("import { add } from 'math'; add(2, 3);")).toBe("5");
});

test("exported function declaration reaches an importer", () => {
  const vm = new Vm();
  vm.registerModule("fns", "export function double(x) { return x * 2; }");
  expect(vm.run("import { double } from 'fns'; double(4);")).toBe("8");
});

test("default export reaches an importer", () => {
  const vm = new Vm();
  vm.registerModule("ans", "export default 42;");
  expect(vm.run("import ans from 'ans'; ans;")).toBe("42");
});

test("namespace import collects exports", () => {
  const vm = new Vm();
  vm.registerModule("ns", "export const a = 1; export const b = 2;");
  expect(vm.run("import * as m from 'ns'; m.a + m.b;")).toBe("3");
});
