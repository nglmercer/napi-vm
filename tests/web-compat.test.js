import { test, expect } from "bun:test";
import { runCode, Vm } from "../index.js";

// ---------------------------------------------------------------------------
// Web-compat spec gaps: search positions, collection prototypes, computed
// member syntax, and `defineProperty` redefinition rules. Real-world libraries
// (linkedom, `entities`, `css-select`) need all of these to run as guest code.
// ---------------------------------------------------------------------------

test("Array search methods honor fromIndex", () => {
  expect(runCode("[1, 2, 1, 2].indexOf(2, 2);")).toBe("3");
  expect(runCode("[1, 2, 1, 2].indexOf(2, -2);")).toBe("3");
  expect(runCode("[1, 2, 1, 2].indexOf(1, 9);")).toBe("-1");
  expect(runCode("[1, 2, 1].lastIndexOf(1, 1);")).toBe("0");
  expect(runCode("[1, 2, 1].lastIndexOf(2, 1);")).toBe("1");
  expect(runCode("[1, 2, 1].lastIndexOf(1, -2);")).toBe("0");
  expect(runCode("[1, 2, 1].lastIndexOf(1, 9);")).toBe("2");
  expect(runCode("[1, 2, 3].includes(1, 1);")).toBe("false");
  expect(runCode("[1, 2, 3].includes(3, -1);")).toBe("true");
  expect(runCode("[NaN].includes(NaN);")).toBe("true");
  expect(runCode("[NaN].indexOf(NaN);")).toBe("-1");
  // Omitted positions keep the old behavior.
  expect(runCode("[1, 2, 1].indexOf(1);")).toBe("0");
  expect(runCode("[1, 2, 1].lastIndexOf(1);")).toBe("2");
  expect(runCode("[1, 2, 3].includes(2);")).toBe("true");
});

test("typed array search methods honor fromIndex", () => {
  expect(runCode("new Uint8Array([1, 2, 1]).indexOf(1, 1);")).toBe("2");
  expect(runCode("new Uint8Array([1, 2, 3]).includes(1, 1);")).toBe("false");
  expect(runCode("new Uint8Array([1, 2, 1]).lastIndexOf(1, 1);")).toBe("0");
});

test("String search methods honor the position argument", () => {
  expect(runCode("'abcabc'.indexOf('b', 2);")).toBe("4");
  expect(runCode("'abcabc'.indexOf('b', -4);")).toBe("4");
  expect(runCode("'abc'.indexOf('b', 9);")).toBe("-1");
  expect(runCode("'abcabc'.lastIndexOf('b', 3);")).toBe("1");
  expect(runCode("'abc'.lastIndexOf('b', 9);")).toBe("1");
  expect(runCode("'abc'.includes('b', 2);")).toBe("false");
  expect(runCode("'abc'.startsWith('b', 1);")).toBe("true");
  expect(runCode("'abc'.startsWith('a', 1);")).toBe("false");
  expect(runCode("'abc'.endsWith('b', 2);")).toBe("true");
  expect(runCode("'abc'.endsWith('c', 2);")).toBe("false");
  // Empty needles still match at the clamped position.
  expect(runCode("'abc'.indexOf('', 2);")).toBe("2");
  expect(runCode("'abc'.lastIndexOf('', 2);")).toBe("2");
  // Omitted positions keep the old behavior.
  expect(runCode("'abcb'.indexOf('b');")).toBe("1");
  expect(runCode("'abcb'.lastIndexOf('b');")).toBe("3");
  expect(runCode("'hello'.includes('ell');")).toBe("true");
});

test("String indexOf reports character offsets, like lastIndexOf", () => {
  expect(runCode("'éé'.indexOf('é', 1);")).toBe("1");
  expect(runCode("'éé'.lastIndexOf('é', 0);")).toBe("0");
});

test("Map/Set/WeakMap/WeakSet expose .prototype", () => {
  for (const name of ["Map", "Set", "WeakMap", "WeakSet"]) {
    expect(runCode(`typeof ${name}.prototype;`)).toBe("object");
  }
  expect(runCode("typeof Set.prototype.add;")).toBe("function");
  expect(runCode("typeof Map.prototype.get;")).toBe("function");
  expect(runCode("const { add } = Set.prototype; const s = new Set(); add.call(s, 1); s.size;")).toBe("1");
});

test("classes can extend Map and Set", () => {
  expect(runCode("class S extends Set {} const s = new S(); s.add(1); s.size;")).toBe("1");
  expect(runCode("class M extends Map {} const m = new M(); m.set('a', 1); m.get('a');")).toBe("1");
});

test("destructuring accepts computed keys", () => {
  expect(runCode("const k = 'a'; const { [k]: v } = { a: 42 }; v;")).toBe("42");
  expect(runCode("const k = 'a'; const { [k]: v = 1 } = {}; v;")).toBe("1");
  expect(runCode("const { ['a' + 'b']: v } = { ab: 7 }; v;")).toBe("7");
  expect(runCode("const f = ({ [Symbol.iterator]: it }) => typeof it; f({});")).toBe("undefined");
  expect(runCode("function f({ [0]: first }) { return first; } f({ 0: 'x' });")).toBe("x");
  expect(runCode("const out = []; for (const { [0]: v } of [{ 0: 1 }, { 0: 2 }]) out.push(v); out.join(',');")).toBe("1,2");
  expect(runCode("const K = 'x'; const { [K]: a, plain } = { x: 1, plain: 2 }; a + plain;")).toBe("3");
  expect(runCode("const { extends: e } = { extends: 5 }; e;")).toBe("5");
  expect(runCode("const { 'a-b': v } = { 'a-b': 6 }; v;")).toBe("6");
  expect(() => runCode("const { extends } = {};")).toThrow();
  expect(() => runCode("const { [k] } = {};")).toThrow();
});

test("C-style for accepts a pattern head with trailing declarators", () => {
  expect(runCode("let s = ''; for (let {a} = {a: 1}, i = 0; i < 2; i++) s += a + i; s;")).toBe("12");
  expect(runCode("let s = ''; for (const [x] = [5];;) { s = x; break; } s;")).toBe("5");
  expect(runCode("for (var {q} = {q: 9};;) { break; } q;")).toBe("9");
  expect(runCode("let n = 0; for (let {x} = {x: 3}, i = 0; i < x; i++) n++; n;")).toBe("3");
});

test("classes accept computed, string, and numeric member names", () => {
  expect(runCode("const k = 'm'; class A { [k]() { return 1; } } new A().m();")).toBe("1");
  expect(runCode("class A { [Symbol.iterator]() { return 42; } } new A()[Symbol.iterator]();")).toBe("42");
  expect(runCode("const k = 'v'; class A { get [k]() { return 3; } } new A().v;")).toBe("3");
  expect(runCode("let seen; class A { set [0](v) { seen = v; } } new A()[0] = 9; seen;")).toBe("9");
  expect(runCode("const k = 's'; class A { static [k]() { return 4; } } A.s();")).toBe("4");
  expect(runCode("class A { 'm'() { return 5; } } new A().m();")).toBe("5");
  expect(runCode("class A { 0() { return 6; } } new A()[0]();")).toBe("6");
  expect(runCode("class A { [1 + 1]() { return 7; } } new A()[2]();")).toBe("7");
});

test("module bodies hoist function declarations like scripts", () => {
  const vm = new Vm();
  try {
    vm.defineModule("hoist", "const v = f(); function f() { return 41; } export default v;");
    expect(vm.run('import v from "hoist"; v;')).toBe("41");
    vm.defineModule(
      "hoist-obj",
      "const o = { h: g() }; function g() { return 7; } export default o.h;",
    );
    expect(vm.run('import x from "hoist-obj"; x;')).toBe("7");
  } finally {
    vm.dispose();
  }
});

test("Proxy accepts globalThis as a target", () => {
  expect(runCode("const p = new Proxy(globalThis, {}); typeof p;")).toBe("object");
  expect(runCode("globalThis.__marker = 41; const p = new Proxy(globalThis, {}); p.__marker;")).toBe(
    "41",
  );
  expect(
    runCode(
      "const seen = []; const p = new Proxy(globalThis, { get(t, k) { seen.push(k); return t[k]; } }); p.Array; seen.join();",
    ),
  ).toBe("Array");
});

test("defineProperty may narrow writable on a non-configurable property", () => {
  expect(runCode("function C() {} Object.defineProperty(C, 'prototype', { writable: false }); typeof C.prototype;")).toBe(
    "object",
  );
  // Babel's _createClass shape: define methods, then lock the prototype.
  expect(
    runCode(`
      function C() {}
      Object.defineProperty(C.prototype, 'm', { value: function () { return 8; }, enumerable: false, configurable: true, writable: true });
      Object.defineProperty(C, 'prototype', { writable: false });
      new C().m();
    `),
  ).toBe("8");
  // Genuinely incompatible redefinitions still throw.
  expect(() =>
    runCode("function C() {} Object.defineProperty(C, 'prototype', { writable: false }); Object.defineProperty(C, 'prototype', { enumerable: true });"),
  ).toThrow();
  expect(() =>
    runCode("function C() {} Object.defineProperty(C, 'prototype', { writable: false }); Object.defineProperty(C, 'prototype', { value: 1 });"),
  ).toThrow();
});

// ---------------------------------------------------------------------------
// Gaps found while proving linkedom end to end (P3): destructuring must read
// through getters/prototypes/proxies, same-type `==` must match `===`,
// functions falling off the end return `undefined`, and push/pop are generic.
// ---------------------------------------------------------------------------

test("object destructuring reads through the normal member path", () => {
  // Getters run instead of leaking the getter function.
  expect(
    runCode(`const o = {};
      Object.defineProperty(o, "a", { get() { return 7; }, enumerable: true, configurable: true });
      const { a } = o; a;`),
  ).toBe("7");
  // Prototype chain applies (`const { document } = parseHTML(...)` shape).
  expect(runCode(`const o2 = Object.create({ document: 9 }); const { document } = o2; document;`)).toBe("9");
  // Proxies are honored.
  expect(runCode(`const { document } = new Proxy({ document: 5 }, {}); document;`)).toBe("5");
  // Destructuring null/undefined throws instead of yielding undefined.
  expect(() => runCode(`const { a } = null;`)).toThrow();
  expect(() => runCode(`const { a } = undefined;`)).toThrow();
  // `{ ...rest }` invokes getters and skips non-enumerable own props.
  expect(
    runCode(`const o = { a: 1 };
      Object.defineProperty(o, "g", { get() { return 2; }, enumerable: true, configurable: true });
      Object.defineProperty(o, "hidden", { value: 9 });
      const { a, ...r } = o; a + "|" + r.g + "|" + Object.keys(r).join(",");`),
  ).toBe("1|2|g");
});

test("loose equality with same-type operands matches strict equality", () => {
  expect(runCode(`null == null;`)).toBe("true");
  expect(runCode(`undefined == undefined;`)).toBe("true");
  expect(runCode(`null != null;`)).toBe("false");
  expect(runCode(`const o = {}; o == o;`)).toBe("true");
  expect(runCode(`const a = [1]; a == a;`)).toBe("true");
  expect(runCode(`({} == {});`)).toBe("false");
  // Mixed-type coercions still apply.
  expect(runCode(`1 == "1";`)).toBe("true");
  expect(runCode(`0 == false;`)).toBe("true");
  expect(runCode(`null == undefined;`)).toBe("true");
});

test("falling off the end of a function returns undefined", () => {
  expect(runCode(`function f() { 99; } f();`)).toBe("undefined");
  expect(runCode(`const f = () => { 42; }; f();`)).toBe("undefined");
  // Concise arrow bodies still return their value.
  expect(runCode(`const f = () => 42; f();`)).toBe("42");
  // `new` on a class whose constructor ends with an object assignment
  // yields the instance, not the assigned object.
  expect(
    runCode(`class T { constructor() { this.x = 1; this.inner = { a: 1 }; } }
      const t = new T(); (t instanceof T) + "|" + t.x + "|" + t.inner.a;`),
  ).toBe("true|1|1");
  // Explicit object returns from function constructors are still adopted.
  expect(runCode(`function G() { this.x = 1; return { hi: 1 }; } new G().hi;`)).toBe("1");
});

test("Array push/pop are generic over non-array receivers", () => {
  // `class NodeList extends Array` instances accumulate pushed items.
  expect(
    runCode(`class NL extends Array {}
      const nl = new NL(); nl.push("a"); nl.push("b"); nl.length + "|" + nl[0] + nl[1];`),
  ).toBe("2|ab");
  // Plain array-likes work through .call.
  expect(runCode(`const o = { length: 0 }; Array.prototype.push.call(o, "x", "y"); o.length + "|" + o[0];`)).toBe(
    "2|x",
  );
  expect(
    runCode(`const o = { 0: "a", 1: "b", length: 2 }; const v = Array.prototype.pop.call(o); v + "|" + o.length;`),
  ).toBe("b|1");
  expect(runCode(`const o = { length: 0 }; Array.prototype.pop.call(o);`)).toBe("undefined");
  // Real arrays keep the fast path.
  expect(runCode(`const a = [1]; a.push(2, 3); a.length + "|" + a.join(",");`)).toBe("3|1,2,3");
  expect(runCode(`const a = [1, 2]; a.pop() + "|" + a.length;`)).toBe("2|1");
  // Nullish receivers throw like the spec's ToObject.
  expect(() => runCode(`Array.prototype.push.call(null, 1);`)).toThrow();
  expect(() => runCode(`Array.prototype.pop.call(undefined);`)).toThrow();
});
